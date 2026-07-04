use std::collections::HashMap;
use std::hash::Hash;

use glam::Vec2;
use palantir::{Rect, ResponseState, Ui};
use scenarium::graph::NodeId;

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
    /// Data-port circles, keyed by [`PortRef`].
    ports: PortLayer<PortRef>,
    /// Emitter event glyphs (the white triangles under a node's outputs),
    /// keyed by [`EventRef`]. The drag source for subscription wires.
    events: PortLayer<EventRef>,
    /// Subscription pins (the top-left triangle on terminal nodes), keyed
    /// by node — a subscription is whole-node, so one pin per node. The
    /// drop target for subscription wires.
    subs: PortLayer<NodeId>,
}

/// One key-domain's port snapshot, split into two tiers by lifetime:
///
/// - `live` is cleared and rebuilt every frame from last frame's responses.
/// - `offsets` (per-widget intra-node offset, `widget_rect.center -
///   node_rect.min`) is kept **across frames and tab switches**. An offset
///   is layout-stable (it depends only on the node's content, not its
///   position), so when a graph is shown again — e.g. the frame after
///   switching back to its tab, where none of its widgets recorded last
///   frame — centers still resolve from `node.pos + cached_offset` and
///   connections draw on that first frame instead of popping in one frame
///   late. Keyed by the globally-unique domain key, so it spans every open
///   graph; on doc reload the whole `GraphUI` (and this cache) is dropped.
#[derive(Debug)]
struct PortLayer<K> {
    live: HashMap<K, PortInfo>,
    offsets: HashMap<K, Vec2>,
}

impl<K> Default for PortLayer<K> {
    fn default() -> Self {
        Self {
            live: HashMap::new(),
            offsets: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash + Copy> PortLayer<K> {
    /// Snapshot one widget into `live`, refreshing its persistent offset.
    fn record(&mut self, key: K, r: ResponseState, node_min: Option<Vec2>, node_pos: Vec2) {
        let info = snapshot(r, node_min, node_pos, key, &mut self.offsets);
        self.live.insert(key, info);
    }

    /// Canvas-local pre-transform center, or `None` when the widget or its
    /// parent node hasn't measured yet.
    fn center(&self, key: K) -> Option<Vec2> {
        self.live.get(&key)?.layout_center
    }

    /// `true` when `pointer` (screen coords) falls inside this widget's
    /// post-transform/clip rect.
    fn contains_pointer(&self, key: K, pointer: Vec2) -> bool {
        self.live
            .get(&key)
            .and_then(|i| i.screen_rect)
            .is_some_and(|r| r.contains(pointer))
    }

    /// `true` on the one-frame edge of a drag-start on this widget.
    fn drag_started(&self, key: K) -> bool {
        self.live.get(&key).is_some_and(|i| i.drag_started)
    }

    /// `true` while a drag started on this widget is still live.
    fn dragging(&self, key: K) -> bool {
        self.live.get(&key).is_some_and(|i| i.dragging)
    }

    /// `true` when this widget should paint with its hover color.
    fn is_hovered(&self, key: K) -> bool {
        self.live.get(&key).is_some_and(|i| i.hovered)
    }

    /// Force the hover flag on (idempotent) — the active drag's snap target,
    /// which palantir's drag-capture suppression hides from `response.hovered`.
    fn set_hovered(&mut self, key: K) {
        if let Some(info) = self.live.get_mut(&key) {
            info.hovered = true;
        }
    }
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
        self.ports.live.clear();
        self.events.live.clear();
        self.subs.live.clear();
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
                    self.ports.record(port, r, node_min, n.pos);
                }
            }
            // Emitter event glyphs, drag sources for subscription wires.
            for event_idx in 0..n.events.len as usize {
                let ev = EventRef {
                    node_id: n.id,
                    event_idx,
                };
                let r = ui.response_for(event_glyph_wid(n.id, event_idx));
                self.events.record(ev, r, node_min, n.pos);
            }
            // The subscription pin only exists on terminal nodes (only they
            // render one — see `header::subscription_glyph`).
            if n.terminal {
                let r = ui.response_for(subscription_glyph_wid(n.id));
                self.subs.record(n.id, r, node_min, n.pos);
            }
        }
    }

    /// Canvas-local pre-transform port center. `None` when the port
    /// or its parent node hasn't been measured yet.
    pub(crate) fn center_canvas_local(&self, p: PortRef) -> Option<Vec2> {
        self.ports.center(p)
    }

    /// `true` when `pointer` (screen coords) falls inside this port's
    /// post-transform/clip rect.
    pub(crate) fn contains_pointer(&self, p: PortRef, pointer: Vec2) -> bool {
        self.ports.contains_pointer(p, pointer)
    }

    /// `true` on the one-frame edge of a drag-start on this port.
    pub(crate) fn drag_started(&self, p: PortRef) -> bool {
        self.ports.drag_started(p)
    }

    /// `true` while a drag started on this port is still live.
    pub(crate) fn dragging(&self, p: PortRef) -> bool {
        self.ports.dragging(p)
    }

    /// `true` when the port should paint with its hover color —
    /// `response.hovered` plus any forced-on override.
    pub(crate) fn is_hovered(&self, p: PortRef) -> bool {
        self.ports.is_hovered(p)
    }

    /// Force the hover flag on (idempotent). Called after
    /// `ConnectionUI::apply` for the active snap target so it lights
    /// up even though palantir's drag-capture suppression hides it
    /// from `response.hovered`.
    pub(crate) fn set_hovered(&mut self, p: PortRef) {
        self.ports.set_hovered(p);
    }

    /// Canvas-local center of an emitter event glyph, or `None` when it
    /// hasn't measured yet.
    pub(crate) fn event_center_canvas_local(&self, e: EventRef) -> Option<Vec2> {
        self.events.center(e)
    }

    /// `true` on the one-frame edge of a drag-start on this event glyph.
    pub(crate) fn event_drag_started(&self, e: EventRef) -> bool {
        self.events.drag_started(e)
    }

    /// `true` while a drag started on this event glyph is still live.
    pub(crate) fn event_dragging(&self, e: EventRef) -> bool {
        self.events.dragging(e)
    }

    /// `true` when an emitter event glyph is hovered (plain mouse-over).
    pub(crate) fn event_is_hovered(&self, e: EventRef) -> bool {
        self.events.is_hovered(e)
    }

    /// `true` when `pointer` (screen coords) falls inside this emitter event
    /// glyph's rect — the snap test for a reverse (subscriber → emitter) drag.
    pub(crate) fn event_contains_pointer(&self, e: EventRef, pointer: Vec2) -> bool {
        self.events.contains_pointer(e, pointer)
    }

    /// Force an emitter event glyph's hover flag on (idempotent) — the
    /// reverse event drag's snap target, mirroring [`Self::set_sub_hovered`].
    pub(crate) fn set_event_hovered(&mut self, e: EventRef) {
        self.events.set_hovered(e);
    }

    /// Canvas-local center of a node's subscription pin, or `None` when it
    /// hasn't measured yet (or the node has no pin).
    pub(crate) fn sub_center_canvas_local(&self, node_id: NodeId) -> Option<Vec2> {
        self.subs.center(node_id)
    }

    /// `true` when `pointer` (screen coords) falls inside this node's
    /// subscription-pin rect.
    pub(crate) fn sub_contains_pointer(&self, node_id: NodeId, pointer: Vec2) -> bool {
        self.subs.contains_pointer(node_id, pointer)
    }

    /// `true` on the one-frame edge of a drag-start on this subscription pin —
    /// the reverse (subscriber → emitter) event drag's latch.
    pub(crate) fn sub_drag_started(&self, node_id: NodeId) -> bool {
        self.subs.drag_started(node_id)
    }

    /// `true` while a drag started on this subscription pin is still live.
    /// Read on the start pin to detect release.
    pub(crate) fn sub_dragging(&self, node_id: NodeId) -> bool {
        self.subs.dragging(node_id)
    }

    /// `true` when a node's subscription pin should paint highlighted — set
    /// by the canvas for the active drag's snap target (palantir's
    /// drag-capture suppression otherwise hides it from `response.hovered`).
    pub(crate) fn sub_is_hovered(&self, node_id: NodeId) -> bool {
        self.subs.is_hovered(node_id)
    }

    /// Force a subscription pin's hover flag on (idempotent) — the event
    /// drag's snap target, mirroring [`Self::set_hovered`] for data ports.
    pub(crate) fn set_sub_hovered(&mut self, node_id: NodeId) {
        self.subs.set_hovered(node_id);
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
