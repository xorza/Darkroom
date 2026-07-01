use std::collections::HashMap;
use std::hash::Hash;

use glam::Vec2;
use palantir::{Rect, ResponseState, Ui};
use scenarium::prelude::NodeId;

use crate::gui::canvas::node_ports;
use crate::gui::node::header::subscription_glyph_wid;
use crate::gui::node::node_widget_id;
use crate::gui::node::port_row::{event_glyph_wid, port_circle_wid};
use crate::gui::scene::Scene;
use crate::gui::{EventRef, PortKind, PortRef};

/// Per-frame snapshot of the four `ResponseState` fields downstream
/// consumers actually read. Built once at the top of
/// [`crate::gui::canvas::GraphUI::frame`] by polling [`Ui::response_for`]
/// on each port's deterministic [`port_circle_wid`]. Sized to the four
/// bytes-and-bits we use (`layout_rect.center()`, `rect`, two edge bools)
/// instead of the full `ResponseState`.
///
/// Ports that haven't recorded yet (first frame after a node spawns)
/// have an entry with `layout_center` / `screen_rect` = `None`. The
/// edge bools default to `false` for them, so `drag_started` / `dragging`
/// queries are correct without a presence check.
#[derive(Default, Debug)]
pub(crate) struct PortFrame {
    map: HashMap<PortRef, PortInfo>,
    /// Emitter event glyphs (the white triangles under a node's outputs),
    /// keyed by [`EventRef`]. The drag source for subscription wires.
    events: HashMap<EventRef, PortInfo>,
    /// Subscription pins (the top-left triangle on terminal nodes), keyed
    /// by node — a subscription is whole-node, so one pin per node. The
    /// drop target for subscription wires.
    subs: HashMap<NodeId, PortInfo>,
    /// Per-port intra-node offset (`port_rect.center - node_rect.min`),
    /// kept **across frames and tab switches**. A port's offset is
    /// layout-stable (it only depends on the node's content, not its
    /// position), so when a graph is shown again — e.g. the frame after
    /// switching back to its tab, where none of its widgets recorded
    /// last frame — we still resolve port centers from `node.pos +
    /// cached_offset` and connections draw on that first frame instead
    /// of popping in one frame late. Keyed by the globally-unique
    /// `PortRef`, so it naturally spans every open graph.
    offsets: HashMap<PortRef, Vec2>,
    /// Intra-node offsets for event glyphs — same role as `offsets`.
    event_offsets: HashMap<EventRef, Vec2>,
    /// Intra-node offsets for subscription pins — same role as `offsets`.
    sub_offsets: HashMap<NodeId, Vec2>,
}

#[derive(Clone, Copy, Debug)]
struct PortInfo {
    /// Port-circle center in canvas-local (inner-canvas pre-transform)
    /// coords. Computed as `node.pos + port_offset_within_node` so a
    /// just-moved node's curves anchor on this frame's port positions
    /// instead of last frame's stale `response.layout_rect`. `None`
    /// when either the port or its parent node hasn't measured yet.
    layout_center: Option<Vec2>,
    /// Post-transform/clip screen rect for pointer hit-test (snap).
    /// Bypasses palantir's drag-capture hover suppression by reading
    /// geometry directly.
    screen_rect: Option<Rect>,
    /// `true` when the port should paint with its hover color. Filled
    /// from `response.hovered` in `rebuild`; an active connection
    /// drag's snap target gets it forced on via `set_hovered` after
    /// `ConnectionUI::apply` (palantir's drag-capture suppression
    /// otherwise hides the snap target from `response.hovered`).
    hovered: bool,
    /// One-frame edge: pointer-down → drag latched on this port this
    /// frame. Drives connection-drag start detection.
    drag_started: bool,
    /// Continuous: a drag is currently live on this port
    /// (`drag_delta` is `Some` OR `drag_started` fired this frame).
    /// Read on the start port to detect release.
    dragging: bool,
}

impl PortFrame {
    pub(crate) fn rebuild(&mut self, ui: &Ui, scene: &Scene) {
        self.map.clear();
        self.events.clear();
        self.subs.clear();
        for n in &scene.nodes {
            // Port offsets within a node are stable; the node's
            // canvas-local position changes when the user drags. Take
            // `port_offset = port_rect.center - node_rect.min` from
            // last frame's layout (same frame for both, so any
            // ancestor-shared canvas-origin term cancels) and combine
            // with this frame's `n.pos` — curves anchor on the moved
            // node's *current* port positions, not last frame's.
            let node_min = ui
                .response_for(node_widget_id(n.id))
                .layout_rect
                .map(|r| r.min);
            for kind in [PortKind::Input, PortKind::Output] {
                for port in node_ports(n, kind) {
                    let r = ui.response_for(port_circle_wid(port));
                    let info = snapshot(r, node_min, n.pos, port, &mut self.offsets);
                    self.map.insert(port, info);
                }
            }
            // Emitter event glyphs, drag sources for subscription wires.
            for event_idx in 0..n.events.len as usize {
                let ev = EventRef {
                    node_id: n.id,
                    event_idx,
                };
                let r = ui.response_for(event_glyph_wid(n.id, event_idx));
                let info = snapshot(r, node_min, n.pos, ev, &mut self.event_offsets);
                self.events.insert(ev, info);
            }
            // The subscription pin only exists on terminal nodes (only they
            // render one — see `header::subscription_glyph`).
            if n.terminal {
                let r = ui.response_for(subscription_glyph_wid(n.id));
                let info = snapshot(r, node_min, n.pos, n.id, &mut self.sub_offsets);
                self.subs.insert(n.id, info);
            }
        }
        // `offsets` deliberately accumulates across every tab the user
        // has touched this session — a previous retain-by-current-scene
        // pass dropped every other tab's entries on a tab switch, so
        // the frame after the switch hit an empty cache and skipped
        // every connection. Each entry is tiny (16 bytes), so the
        // session-long cap is fine; on doc reload the whole `GraphUI`
        // is dropped, taking the cache with it.
    }

    /// Canvas-local pre-transform port center. `None` when the port
    /// or its parent node hasn't been measured yet.
    pub(crate) fn center_canvas_local(&self, p: PortRef) -> Option<Vec2> {
        self.map.get(&p)?.layout_center
    }

    /// `true` when `pointer` (screen coords) falls inside this port's
    /// post-transform/clip rect.
    pub(crate) fn contains_pointer(&self, p: PortRef, pointer: Vec2) -> bool {
        self.map
            .get(&p)
            .and_then(|i| i.screen_rect)
            .is_some_and(|r| r.contains(pointer))
    }

    /// `true` on the one-frame edge of a drag-start on this port.
    pub(crate) fn drag_started(&self, p: PortRef) -> bool {
        self.map.get(&p).is_some_and(|i| i.drag_started)
    }

    /// `true` while a drag started on this port is still live.
    pub(crate) fn dragging(&self, p: PortRef) -> bool {
        self.map.get(&p).is_some_and(|i| i.dragging)
    }

    /// `true` when the port should paint with its hover color —
    /// `response.hovered` plus any forced-on override.
    pub(crate) fn is_hovered(&self, p: PortRef) -> bool {
        self.map.get(&p).is_some_and(|i| i.hovered)
    }

    /// Force the hover flag on (idempotent). Called after
    /// `ConnectionUI::apply` for the active snap target so it lights
    /// up even though palantir's drag-capture suppression hides it
    /// from `response.hovered`.
    pub(crate) fn set_hovered(&mut self, p: PortRef) {
        if let Some(info) = self.map.get_mut(&p) {
            info.hovered = true;
        }
    }

    /// Canvas-local center of an emitter event glyph, or `None` when it
    /// hasn't measured yet.
    pub(crate) fn event_center_canvas_local(&self, e: EventRef) -> Option<Vec2> {
        self.events.get(&e)?.layout_center
    }

    /// `true` on the one-frame edge of a drag-start on this event glyph.
    pub(crate) fn event_drag_started(&self, e: EventRef) -> bool {
        self.events.get(&e).is_some_and(|i| i.drag_started)
    }

    /// `true` while a drag started on this event glyph is still live.
    pub(crate) fn event_dragging(&self, e: EventRef) -> bool {
        self.events.get(&e).is_some_and(|i| i.dragging)
    }

    /// `true` when an emitter event glyph is hovered (plain mouse-over).
    pub(crate) fn event_is_hovered(&self, e: EventRef) -> bool {
        self.events.get(&e).is_some_and(|i| i.hovered)
    }

    /// `true` when `pointer` (screen coords) falls inside this emitter event
    /// glyph's rect — the snap test for a reverse (subscriber → emitter) drag.
    pub(crate) fn event_contains_pointer(&self, e: EventRef, pointer: Vec2) -> bool {
        self.events
            .get(&e)
            .and_then(|i| i.screen_rect)
            .is_some_and(|r| r.contains(pointer))
    }

    /// Force an emitter event glyph's hover flag on (idempotent) — the
    /// reverse event drag's snap target, mirroring [`Self::set_sub_hovered`].
    pub(crate) fn set_event_hovered(&mut self, e: EventRef) {
        if let Some(info) = self.events.get_mut(&e) {
            info.hovered = true;
        }
    }

    /// Canvas-local center of a node's subscription pin, or `None` when it
    /// hasn't measured yet (or the node has no pin).
    pub(crate) fn sub_center_canvas_local(&self, node_id: NodeId) -> Option<Vec2> {
        self.subs.get(&node_id)?.layout_center
    }

    /// `true` when `pointer` (screen coords) falls inside this node's
    /// subscription-pin rect.
    pub(crate) fn sub_contains_pointer(&self, node_id: NodeId, pointer: Vec2) -> bool {
        self.subs
            .get(&node_id)
            .and_then(|i| i.screen_rect)
            .is_some_and(|r| r.contains(pointer))
    }

    /// `true` on the one-frame edge of a drag-start on this subscription pin —
    /// the reverse (subscriber → emitter) event drag's latch.
    pub(crate) fn sub_drag_started(&self, node_id: NodeId) -> bool {
        self.subs.get(&node_id).is_some_and(|i| i.drag_started)
    }

    /// `true` while a drag started on this subscription pin is still live.
    /// Read on the start pin to detect release.
    pub(crate) fn sub_dragging(&self, node_id: NodeId) -> bool {
        self.subs.get(&node_id).is_some_and(|i| i.dragging)
    }

    /// `true` when a node's subscription pin should paint highlighted — set
    /// by the canvas for the active drag's snap target (palantir's
    /// drag-capture suppression otherwise hides it from `response.hovered`).
    pub(crate) fn sub_is_hovered(&self, node_id: NodeId) -> bool {
        self.subs.get(&node_id).is_some_and(|i| i.hovered)
    }

    /// Force a subscription pin's hover flag on (idempotent) — the event
    /// drag's snap target, mirroring [`Self::set_hovered`] for data ports.
    pub(crate) fn set_sub_hovered(&mut self, node_id: NodeId) {
        if let Some(info) = self.subs.get_mut(&node_id) {
            info.hovered = true;
        }
    }
}

/// Snapshot one widget's [`ResponseState`] into a [`PortInfo`]: refresh the
/// intra-node offset from this frame's rects when both recorded, else fall
/// back to the cached offset so a just-shown graph still anchors. The center
/// is `node_pos + offset` so a moved node's glyph tracks its current
/// position. Shared by data ports, event glyphs, and subscription pins.
fn snapshot<K: Eq + Hash + Copy>(
    r: ResponseState,
    node_min: Option<Vec2>,
    node_pos: Vec2,
    key: K,
    offsets: &mut HashMap<K, Vec2>,
) -> PortInfo {
    let fresh_offset = match (r.layout_rect, node_min) {
        (Some(rect), Some(node_min)) => Some(rect.center() - node_min),
        _ => None,
    };
    if let Some(offset) = fresh_offset {
        offsets.insert(key, offset);
    }
    let layout_center = fresh_offset
        .or_else(|| offsets.get(&key).copied())
        .map(|offset| node_pos + offset);
    PortInfo {
        layout_center,
        screen_rect: r.rect,
        hovered: r.hovered,
        drag_started: r.drag_started(),
        dragging: r.drag_started() || r.drag_delta().is_some(),
    }
}
