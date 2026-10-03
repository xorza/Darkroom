//! [`GlyphDrag`]: one in-flight wire drag between two glyph layers.

use glam::Vec2;
use palantir::Ui;
use scenarium::NodeId;

use crate::core::document::node_key::NodeKey;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::frame::geometry::PortLayer;

/// One in-flight wire drag: the glyph the press latched (`from`, a key in
/// layer `A`) and the compatible glyph currently under the pointer (`snap`, a
/// key in layer `B`).
///
/// Identity-only — both ends resolve their position out of `CanvasGeometry`
/// every frame, so a drag survives node moves and relayouts. The direction of
/// an event drag flips the two domains, which is why `A` and `B` are separate
/// parameters rather than one: a subscriber-started drag is a
/// `GlyphDrag<NodeId, EventRef>` and an emitter-started one the reverse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct GlyphDrag<A, B> {
    /// The glyph the press latched. Fixed for the drag's whole life.
    pub(crate) from: A,
    /// Compatible glyph currently under the pointer — the preview's snap end,
    /// the forced hover highlight, and what a release commits against.
    pub(crate) snap: Option<B>,
}

impl<A: NodeKey, B: NodeKey> GlyphDrag<A, B> {
    /// A drag off `from` with nothing snapped yet.
    pub(crate) fn new(from: A) -> Self {
        Self { from, snap: None }
    }

    /// Latch on the glyph that began a drag this frame, when `accepts` takes
    /// it; `None` when no press landed on one.
    pub(crate) fn latch(layer: &PortLayer<A>, accepts: impl FnOnce(A) -> bool) -> Option<Self> {
        layer
            .started_drag()
            .filter(|&key| accepts(key))
            .map(Self::new)
    }

    /// The node the fixed end hangs off, whose disappearance (undo, a breaker
    /// swipe) ends the gesture.
    pub(crate) fn node(self) -> NodeId {
        self.from.node()
    }

    /// Whether the press that latched this drag is still held.
    /// `PortLayer::dragging` rolls up `drag_delta().is_some() ||
    /// drag_started()`, so its transition to `false` is the release edge.
    pub(crate) fn held(self, layer: &PortLayer<A>) -> bool {
        layer.dragging(self.from)
    }

    /// The moving end of the preview curve: the snapped glyph's center once
    /// the drag has a target, else the bare pointer in canvas-world coords.
    /// `None` on a frame where neither resolves (pointer off-window, or a snap
    /// target that hasn't measured yet) — the preview simply doesn't paint
    /// that frame. `canvas_origin` is the inner canvas's pre-transform origin.
    pub(crate) fn free_end(
        self,
        ui: &mut Ui,
        graph_ctx: GraphCtx<'_>,
        canvas_origin: Vec2,
        layer: &PortLayer<B>,
    ) -> Option<Vec2> {
        match self.snap {
            Some(key) => layer.center(key),
            None => ui
                .pointer_pos()
                .map(|p| graph_ctx.viewport().to_world(p - canvas_origin)),
        }
    }
}
