use std::collections::HashMap;
use std::fmt::Debug;

use glam::Vec2;
use palantir::{Rect, Size, Ui};
use scenarium::NodeId;

use crate::core::document::graph_view::GraphView;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::frame::canvas_geometry::port_layer::{NodePlace, PortLayer};
use crate::gui::pane::graph_ui::wid;
use crate::gui::port_ref::{PortKind, PortRef};
use scenarium::EventPort;

pub(crate) mod port_layer;

/// The canvas's response-derived geometry: a per-frame snapshot of every port-ish glyph plus the
/// cross-frame node-size cache, all filled by one [`Self::rebuild`] pass polling
/// [`Ui::response_for`] on deterministic widget ids. Rebuilt in
/// [`crate::gui::pane::graph_ui::GraphUi::prepass`], its centers moved by
/// [`Self::reposition`] to where the record finds the nodes, and reused by
/// [`crate::gui::pane::graph_ui::GraphUi::draw`]. Each glyph snapshot keeps the few fields read
/// of it instead of the whole `ResponseState`.
///
/// Ports that haven't recorded yet (first frame after a node spawns)
/// have an entry with `layout_center` / `screen_rect` = `None`. The
/// edge bools default to `false` for them, so `drag_started` / `dragging`
/// queries are correct without a presence check.
///
/// The glyph domains are read directly by consumers via the public
/// fields — `geometry.ports.center(p)`, `geometry.events.drag_started(e)` —
/// rather than through a per-domain forwarding method each; node bodies
/// resolve through [`Self::node_world_rect`] and [`Self::node_screen_rect`].
///
/// **The one place a node body's rect is polled.** [`Self::rebuild`] already
/// reads that response to derive the port offsets, so both frames a caller
/// could want come off the same read: world coords for the gestures that share
/// the canvas's own space (cull, breaker, rubber band, view fitting), screen
/// coords for the ones testing a raw pointer position. Everything else asks
/// here rather than calling `response_for(wid::body(..))` itself, so no
/// two of them can disagree about where a node is.
///
/// **And the one place a port glyph's is.** The same rule, one level down:
/// [`Self::rebuild`] reads each port circle's response for its center, and
/// every other reader asks here rather than polling that widget again.
#[derive(Default, Debug)]
pub(crate) struct CanvasGeometry {
    /// Data-port circles, keyed by [`PortRef`].
    pub(crate) ports: PortLayer<PortRef>,
    /// Emitter event glyphs (the white triangles under a node's outputs),
    /// keyed by [`EventPort`]. The drag source for subscription wires.
    pub(crate) events: PortLayer<EventPort>,
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
    /// Only the *size* is cached, never the position: `NodeCtx::pos` is
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

impl CanvasGeometry {
    /// The node's world-space (inner-canvas pre-transform) body rect *this
    /// frame*: the scene's current position combined with the cached measured
    /// size — so a node the document moved under a live gesture (a drag, an
    /// undo) culls, band-hits, and breaker-hits where it is today, not where
    /// it last recorded. `None` until the node's first record.
    pub(crate) fn node_world_rect(&self, node: NodeCtx<'_>) -> Option<Rect> {
        let size = *self.node_sizes.get(&node.node_id)?;
        Some(Rect {
            min: node.pos,
            size,
        })
    }

    /// The node's post-transform, clipped body rect — the frame a raw
    /// `Ui::pointer_pos` lives in, for the release-time drop tests that ask
    /// "did this land on a node?". `None` for a node that didn't record last
    /// frame (never shown, or culled off-screen), which reads as "not here".
    pub(crate) fn node_screen_rect(&self, node_id: NodeId) -> Option<Rect> {
        self.node_screen.get(&node_id).copied()
    }

    /// Whether `pointer` (screen coords) is over any node body at all — the
    /// "released into empty space" half of the wire gesture's drop.
    pub(crate) fn over_any_node(&self, pointer: Vec2) -> bool {
        self.node_screen.values().any(|r| r.contains(pointer))
    }

    /// Move every glyph's center to where its node sits in `view`: the record reads the document
    /// the frame's drain left, a dragged node already moved, where [`Self::rebuild`] read it before
    /// the move. A position, unlike a node's size, is never kept from before it.
    pub(crate) fn reposition(&mut self, view: &GraphView) {
        self.ports.reposition(view);
        self.events.reposition(view);
        self.subs.reposition(view);
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
    /// Run beside the preview store's reconcile when the document's node set
    /// changed: a lookup per cached entry, against a node set that only ever
    /// shrinks by deletion.
    pub(crate) fn retain_nodes(&mut self, keep: impl Fn(NodeId) -> bool) {
        self.ports.retain_nodes(&keep);
        self.events.retain_nodes(&keep);
        self.subs.retain_nodes(&keep);
        self.node_sizes.retain(|id, _| keep(*id));
    }

    /// One walk over every node and port — see the type docs for why the
    /// polls live here: the body poll below is at once the node's cached size
    /// and its screen rect.
    ///
    /// Runs in [`crate::gui::pane::graph_ui::GraphUi::prepass`]. It clears and
    /// refills the per-frame snapshots whole, so nothing carries over from the
    /// frame before.
    pub(crate) fn rebuild(&mut self, ui: &Ui, graph_ctx: GraphCtx<'_>) {
        self.ports.begin_frame();
        self.events.begin_frame();
        self.subs.begin_frame();
        self.node_screen.clear();
        for n in graph_ctx.nodes() {
            // Port offsets within a node are stable; the node's
            // canvas-local position changes when the user drags. Take
            // `port_offset = port_rect.center - node_rect.min` from
            // last frame's layout (same frame for both, so any
            // ancestor-shared canvas-origin term cancels) and combine
            // with this frame's `n.pos` — curves anchor on the moved
            // node's *current* port positions, not last frame's.
            let body = ui.response_for(wid::body(n.node_id));
            if let Some(r) = body.layout_rect {
                self.node_sizes.insert(n.node_id, r.size);
            }
            if let Some(r) = body.rect {
                self.node_screen.insert(n.node_id, r);
            }
            // A node that didn't record has no glyph that did either — they
            // are its descendants — so every one of their responses is the
            // default, and polling them one by one only rediscovers that.
            // Skip straight to the cached-offset reconstruction, which is
            // what those polls would have produced anyway. This is what keeps
            // the pass proportional to what's *on screen*: a culled node
            // costs one lookup, not one per port plus one per event glyph
            // plus one for its pin.
            let Some(place) = body.layout_rect.map(|r| NodePlace {
                layout_min: r.min,
                pos: n.pos,
            }) else {
                self.replay_cached(n);
                continue;
            };
            for kind in [PortKind::Input, PortKind::Output] {
                for port in n.ports(kind) {
                    let r = ui.response_for(wid::port_circle(port));
                    self.ports.record(port, r, place);
                }
            }
            // Emitter event glyphs, drag sources for subscription wires.
            for ev in n.event_ports() {
                let r = ui.response_for(wid::event_glyph(ev));
                self.events.record(ev, r, place);
            }
            // The subscription pin only exists on sink nodes (only they
            // render one — see `header::subscription_glyph`).
            if n.sink() {
                let r = ui.response_for(wid::subscription_glyph(n.node_id));
                self.subs.record(n.node_id, r, place);
            }
        }
    }

    /// Fill in `n`'s glyph snapshots from the persistent offsets alone, for a
    /// node that recorded nothing last frame. Positions still track this
    /// frame's `n.pos`, so a wire leaving the viewport stays anchored to the
    /// off-screen port it runs to; every interaction flag is false, which is
    /// the truth for a widget that isn't on screen to interact with.
    fn replay_cached(&mut self, n: NodeCtx<'_>) {
        for kind in [PortKind::Input, PortKind::Output] {
            for port in n.ports(kind) {
                self.ports.replay(port, n.pos);
            }
        }
        for ev in n.event_ports() {
            self.events.replay(ev, n.pos);
        }
        if n.sink() {
            self.subs.replay(n.node_id, n.pos);
        }
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

        /// Whether the cross-frame size cache holds `id`.
        pub(crate) fn caches_node(&self, id: NodeId) -> bool {
            self.node_sizes.contains_key(&id)
        }
    }
}

#[cfg(test)]
mod tests;
