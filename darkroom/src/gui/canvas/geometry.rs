use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

use glam::Vec2;
use palantir::{Rect, ResponseState, Size, Ui};
use scenarium::NodeId;

use crate::core::document::{PortKind, PortRef};
use crate::gui::EventRef;
use crate::gui::canvas::hits::CanvasHits;
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
///
/// **And the one place a port glyph's is.** The same rule, one level down:
/// [`Self::rebuild`] hands each port's response to
/// [`CanvasHits::note_port`] as it reads it, so a double-click and a port
/// center come off one poll instead of a poll each.
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

/// A glyph key that names the node its glyph hangs off — how a [`PortLayer`]
/// evicts a deleted node's entries, and how a wire drag resolves the pane it
/// belongs to (`Scene::owner`) and notices the node disappearing under it.
/// Every glyph domain the canvas keys on has one: a data port and an emitter
/// event belong to their node, and a subscription pin *is* its node (a
/// subscription is whole-node, so its layer is keyed by `NodeId` directly).
pub(crate) trait GlyphKey: Copy + Eq + Hash + Debug {
    fn node(self) -> NodeId;
}

impl GlyphKey for PortRef {
    fn node(self) -> NodeId {
        self.node_id
    }
}

impl GlyphKey for EventRef {
    fn node(self) -> NodeId {
        self.node_id
    }
}

impl GlyphKey for NodeId {
    fn node(self) -> NodeId {
        self
    }
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

impl<K: GlyphKey> PortLayer<K> {
    /// Snapshot one widget into `live`, refreshing its persistent offset.
    fn record(&mut self, key: K, r: ResponseState, node_min: Option<Vec2>, node_pos: Vec2) {
        let info = snapshot(r, node_min, node_pos, key, &mut self.offsets);
        self.live.insert(key, info);
    }

    /// The entry for a glyph whose node didn't record: its center from the
    /// persistent offset against this frame's `node_pos`, and no interaction
    /// state at all. Equivalent to [`Self::record`] with a default response —
    /// which is exactly what `response_for` would have returned — without
    /// asking for one.
    fn replay(&mut self, key: K, node_pos: Vec2) {
        let Some(offset) = self.offsets.get(&key).copied() else {
            return;
        };
        self.live.insert(
            key,
            PortInfo {
                layout_center: Some(node_pos + offset),
                screen_rect: None,
                hovered: false,
                drag_started: false,
                dragging: false,
            },
        );
    }

    /// Canvas-local pre-transform center, or `None` when the widget or its
    /// parent node hasn't measured yet. `pub(crate)` for the port menu's "Add
    /// preview", which spawns its node off the port circle it was opened on
    /// (`crate::gui::node::port_row`); everything else that reads a center is
    /// inside the canvas.
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

    /// Drop every cross-frame entry whose node `keep` rejects.
    ///
    /// `offsets` and `node_sizes` deliberately outlive the scene — a graph
    /// whose tab is merely closed must still resolve its port centers the
    /// frame it comes back — so absence from the scene is no reason to evict
    /// and [`Self::rebuild`] can't do this itself. Deletion is: a `NodeId` is
    /// never reused, so a node the document has stopped holding will never be
    /// asked about again and its entries are pure ballast.
    ///
    /// Driven off the same `requires_reconcile` signal the preview store
    /// prunes on, so it runs when the document's node set can actually have
    /// shrunk rather than every frame.
    pub(crate) fn retain_nodes(&mut self, keep: impl Fn(NodeId) -> bool) {
        self.ports.offsets.retain(|key, _| keep(key.node()));
        self.events.offsets.retain(|key, _| keep(key.node()));
        self.subs.offsets.retain(|key, _| keep(key.node()));
        self.node_sizes.retain(|id, _| keep(*id));
    }

    /// Fills `hits`' port half on the way through — see the type docs for
    /// why the port polls live here. Runs in
    /// [`crate::gui::canvas::GraphUI::prepass`], after
    /// [`CanvasHits::scan`] has cleared the digest in the navigation
    /// phase, so the two writers never race for a slot.
    pub(super) fn rebuild(&mut self, ui: &Ui, scene: &Scene, hits: &mut CanvasHits) {
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
            // A node that didn't record has no glyph that did either — they
            // are its descendants — so every one of their responses is the
            // default, and polling them one by one only rediscovers that.
            // Skip straight to the cached-offset reconstruction, which is
            // what those polls would have produced anyway. This is what keeps
            // the pass proportional to what's *on screen*: a culled node
            // costs one lookup, not one per port plus one per event glyph
            // plus one for its pin.
            let Some(node_min) = body.layout_rect.map(|r| r.min) else {
                self.replay_cached(n);
                continue;
            };
            for kind in [PortKind::Input, PortKind::Output] {
                for port in n.ports(kind) {
                    let r = ui.response_for(port_circle_wid(port));
                    hits.note_port(ui, port, r);
                    self.ports.record(port, r, Some(node_min), n.pos);
                }
            }
            // Emitter event glyphs, drag sources for subscription wires.
            for ev in n.events() {
                let r = ui.response_for(event_glyph_wid(n.id, ev.event_idx));
                self.events.record(ev, r, Some(node_min), n.pos);
            }
            // The subscription pin only exists on sink nodes (only they
            // render one — see `header::subscription_glyph`).
            if n.sink {
                let r = ui.response_for(subscription_glyph_wid(n.id));
                self.subs.record(n.id, r, Some(node_min), n.pos);
            }
        }
    }

    /// Fill in `n`'s glyph snapshots from the persistent offsets alone, for a
    /// node that recorded nothing last frame. Positions still track this
    /// frame's `n.pos`, so a wire leaving the viewport stays anchored to the
    /// off-screen port it runs to; every interaction flag is false, which is
    /// the truth for a widget that isn't on screen to interact with.
    fn replay_cached(&mut self, n: &SceneNode) {
        for kind in [PortKind::Input, PortKind::Output] {
            for port in n.ports(kind) {
                self.ports.replay(port, n.pos);
            }
        }
        for ev in n.events() {
            self.events.replay(ev, n.pos);
        }
        if n.sink {
            self.subs.replay(n.id, n.pos);
        }
    }
}

/// Snapshot one widget's [`ResponseState`] into a [`PortInfo`]: refresh the
/// intra-node offset from this frame's rects when both recorded, else fall
/// back to the cached offset so a just-shown graph still anchors. The center
/// is `node_pos + offset` so a moved node's glyph tracks its current
/// position. Shared by data ports, event glyphs, and subscription pins.
fn snapshot<K: GlyphKey>(
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
