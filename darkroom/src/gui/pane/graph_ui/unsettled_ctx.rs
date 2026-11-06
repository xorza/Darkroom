//! The canvas's context before its geometry settles.

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas_input::CanvasInput;

/// The graph canvas before its geometry is settled: the graph, plus what this
/// frame's input means on it.
///
/// The level of the context chain, between the [`GraphCtx`] and the
/// [`CanvasCtx`](crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx), which adds the geometry. The
/// three input steps that run *before*
/// [`CanvasGeometry::rebuild`](crate::gui::pane::graph_ui::scene::canvas_geometry::CanvasGeometry::rebuild)
/// — pan/zoom, the group drag's advance and the tie knob's turn — take this, so they read the
/// frame's input without a geometry someone is still writing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct UnsettledCtx<'a> {
    graph_ctx: GraphCtx<'a>,
    input: CanvasInput,
}

impl<'a> UnsettledCtx<'a> {
    pub(super) const fn new(graph_ctx: GraphCtx<'a>, input: CanvasInput) -> Self {
        Self { graph_ctx, input }
    }

    pub(crate) const fn graph_ctx(self) -> GraphCtx<'a> {
        self.graph_ctx
    }

    pub(crate) const fn input(self) -> CanvasInput {
        self.input
    }

    /// Whether this frame's Esc cancels whatever gesture is in flight.
    pub(crate) const fn cancelled(self) -> bool {
        self.input.cancelled()
    }
}
