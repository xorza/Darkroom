use std::collections::HashMap;
use std::hash::Hash;

use glam::Vec2;
use palantir::{Rect, ResponseState, Size, Ui};
use scenarium::NodeId;

use crate::core::document::{PortKind, PortRef};
use crate::gui::EventRef;
use crate::gui::node::header::subscription_glyph_wid;
use crate::gui::node::node_widget_id;
use crate::gui::node::port_row::{event_glyph_wid, port_circle_wid};
use crate::gui::scene::{Scene, SceneNode};

/// The canvas's response-derived geometry: a per-frame snapshot of every
/// port-ish glyph plus the cross-frame node-size cache, all filled by one
/// [`Self::rebuild`] pass polling [`Ui::response_for`] on deterministic
/// widget ids. Rebuilt in [`crate::gui::canvas::GraphUI::prepass`] and
/// reused by `frame`. Each glyph snapshot is sized to the four
/// bytes-and-bits we use (`layout_rect.center()`, `rect`, two edge bools)
/// instead of the full `ResponseState`.
///
/// Ports that haven't recorded yet (first frame after a node spawns)
/// have an entry with `layout_center` / `screen_rect` = `None`. The
/// edge bools default to `false` for them, so `drag_started` / `dragging`
/// queries are correct without a presence check.
///
/// The glyph domains are read directly by consumers via the public
/// fields — `geometry.ports.center(p)`, `geometry.events.drag_started(e)`,
/// `geometry.subs.contains_pointer(id, ptr)` — rather than through a
/// per-domain forwarding method each; node bodies resolve through
/// [`Self::node_world_rect`] and [`Self::node_screen_rect`].
///
/// **The one place a node body's rect is polled.** [`Self::rebuild`] already
/// reads that response to derive the port offsets, so both frames a caller
/// could want come off the same read: world coords for the gestures that share
/// the canvas's own space (cull, breaker, rubber band, view fitting), screen
/// coords for the ones testing a raw pointer position. Everything else asks
/// here rather than calling `response_for(node_widget_id(..))` itself, so no
/// two of them can disagree about where a node is.
#[derive(Default, Debug)]
pub(crate) struct CanvasGeometry {
    /// Data-port circles, keyed by [`PortRef`].
    pub(crate) ports: PortLayer<PortRef>,
    /// Emitter event glyphs (the white triangles under a node's outputs),
    /// keyed by [`EventRef`]. The drag source for subscription wires.
    pub(crate) events: PortLayer<EventRef>,
    /// Subscription pins (the top-left triangle on sink nodes), keyed
    /// by node — a subscription is whole-node, so one pin per node. The
    /// drop target for subscription wires.
    pub(crate) subs: PortLayer<NodeId>,
    /// Last measured body size per node, from the **pre-transform, unclipped**
    /// `layout_rect` and kept **across frames** like `PortLayer::offsets` — a
    /// culled node records no response, so its world rect must come from the
    /// last time it measured, and view-fitting and the rubber band both have
    /// to see off-screen nodes. A node's size depends only on its content, so
    /// a stale entry is off only across a content edit applied while hidden,
    /// and self-heals on next record. Read through [`Self::node_world_rect`].
    ///
    /// Only the *size* is cached, never the position: `SceneNode::pos` is
    /// mirrored pre-record and a cached corner would be a frame behind it,
    /// which is the whole reason the world rect is assembled fresh each read.
    node_sizes: HashMap<NodeId, Size>,
    /// Post-transform, **clipped** body rect per node, refilled every frame
    /// like `PortLayer::live`, for the tests that compare against a raw
    /// pointer position. Read through [`Self::node_screen_rect`] and
    /// [`Self::over_any_node`].
    ///
    /// Not a superset of `node_sizes`, despite carrying a size of its own: it
    /// is scaled by the viewport zoom and cut off at the canvas edge, so it
    /// yields no usable world size for a node at the margin — and it is
    /// absent exactly for the culled nodes that most need one.
    node_screen: HashMap<NodeId, Rect>,
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
pub(crate) struct PortLayer<K> {
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
    pub(crate) fn center(&self, key: K) -> Option<Vec2> {
        self.live.get(&key)?.layout_center
    }

    /// First key in `keys` whose widget contains `pointer` (screen coords),
    /// or `None` — the "which glyph is the in-flight wire hovering" scan
    /// every snap-target search needs, differing only in the candidate
    /// sequence it feeds in and whatever acceptance test it then applies to
    /// the winner. Geometrically at most one glyph sits under the pointer, so
    /// a rejected winner means "no snap", not "keep looking". Sibling of
    /// [`Self::first_drag_started`].
    ///
    /// Tests the post-transform/clip rect, so it sees through palantir's
    /// drag-capture hover suppression.
    pub(super) fn first_containing(
        &self,
        pointer: Vec2,
        mut keys: impl Iterator<Item = K>,
    ) -> Option<K> {
        keys.find(|k| {
            self.live
                .get(k)
                .and_then(|i| i.screen_rect)
                .is_some_and(|r| r.contains(pointer))
        })
    }

    /// First key in `keys` whose drag started this frame, or `None` — the
    /// "which glyph did a fresh drag just latch onto" scan every drag-source
    /// controller (connection/pin/event/subscription) needs, differing only
    /// in the key sequence it feeds in (a node's ports, its events, or its
    /// subscription pin).
    pub(super) fn first_drag_started(&self, mut keys: impl Iterator<Item = K>) -> Option<K> {
        keys.find(|k| self.live.get(k).is_some_and(|i| i.drag_started))
    }

    /// `true` while a drag started on this widget is still live.
    pub(super) fn dragging(&self, key: K) -> bool {
        self.live.get(&key).is_some_and(|i| i.dragging)
    }

    /// `true` when this widget should paint with its hover color.
    pub(crate) fn is_hovered(&self, key: K) -> bool {
        self.live.get(&key).is_some_and(|i| i.hovered)
    }

    /// Force the hover flag on (idempotent) — the active drag's snap target,
    /// which palantir's drag-capture suppression hides from `response.hovered`.
    pub(super) fn set_hovered(&mut self, key: K) {
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

impl CanvasGeometry {
    /// The node's world-space (inner-canvas pre-transform) body rect *this
    /// frame*: the scene's current position combined with the cached measured
    /// size — so a node the document moved under a live gesture (a drag, an
    /// undo) culls, band-hits, and breaker-hits where it is today, not where
    /// it last recorded. `None` until the node's first record.
    pub(crate) fn node_world_rect(&self, node: &SceneNode) -> Option<Rect> {
        let size = *self.node_sizes.get(&node.id)?;
        Some(Rect {
            min: node.pos,
            size,
        })
    }

    /// The node's post-transform, clipped body rect — the frame a raw
    /// `Ui::pointer_pos` lives in, for the release-time drop tests that ask
    /// "did this land on a node?". `None` for a node that didn't record last
    /// frame (never shown, or culled off-screen), which reads as "not here".
    pub(super) fn node_screen_rect(&self, node_id: NodeId) -> Option<Rect> {
        self.node_screen.get(&node_id).copied()
    }

    /// Whether `pointer` (screen coords) is over any node body at all — the
    /// "released into empty space" half of the connection gesture's drop.
    pub(super) fn over_any_node(&self, pointer: Vec2) -> bool {
        self.node_screen.values().any(|r| r.contains(pointer))
    }

    pub(super) fn rebuild(&mut self, ui: &Ui, scene: &Scene) {
        self.ports.live.clear();
        self.events.live.clear();
        self.subs.live.clear();
        self.node_screen.clear();
        for n in scene.nodes.values() {
            // Port offsets within a node are stable; the node's
            // canvas-local position changes when the user drags. Take
            // `port_offset = port_rect.center - node_rect.min` from
            // last frame's layout (same frame for both, so any
            // ancestor-shared canvas-origin term cancels) and combine
            // with this frame's `n.pos` — curves anchor on the moved
            // node's *current* port positions, not last frame's.
            let body = ui.response_for(node_widget_id(n.id));
            if let Some(r) = body.layout_rect {
                self.node_sizes.insert(n.id, r.size);
            }
            if let Some(r) = body.rect {
                self.node_screen.insert(n.id, r);
            }
            let node_min = body.layout_rect.map(|r| r.min);
            for kind in [PortKind::Input, PortKind::Output] {
                for port in n.ports(kind) {
                    let r = ui.response_for(port_circle_wid(port));
                    self.ports.record(port, r, node_min, n.pos);
                }
            }
            // Emitter event glyphs, drag sources for subscription wires.
            for ev in n.events() {
                let r = ui.response_for(event_glyph_wid(n.id, ev.event_idx));
                self.events.record(ev, r, node_min, n.pos);
            }
            // The subscription pin only exists on sink nodes (only they
            // render one — see `header::subscription_glyph`).
            if n.sink {
                let r = ui.response_for(subscription_glyph_wid(n.id));
                self.subs.record(n.id, r, node_min, n.pos);
            }
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
        drag_started: r.left.drag.started(),
        dragging: r.left.drag.started() || r.left.drag.delta().is_some(),
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    impl CanvasGeometry {
        /// Seed the cross-frame size cache directly, standing in for a
        /// past frame's record of the node.
        pub(crate) fn seed_node_size(&mut self, id: NodeId, size: Size) {
            self.node_sizes.insert(id, size);
        }
    }
}
