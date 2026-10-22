//! The canvas's context before its geometry settles: the graph plus what this
//! frame's input means on it.

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::gesture::canvas_gesture::CanvasGesture;

/// What this frame's input means on the canvas, resolved once at the top of
/// [`GraphUi::prepass`](crate::gui::pane::graph_ui::GraphUi::prepass) and read
/// back by every phase after it: which bare-canvas gesture latched, and what
/// Esc does.
///
/// Read there rather than per controller because each is one fact about the
/// frame, and controllers polling them separately is how they grew different
/// guards around the same key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CanvasInput {
    pub(crate) gesture: Option<CanvasGesture>,
    pub(crate) escape: Escape,
}

/// What this frame's Esc does on the canvas. A pressed Esc cancels whatever
/// gesture is in flight; with nothing in flight as the frame starts, it also
/// clears the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Escape {
    #[default]
    Unpressed,
    /// Pressed while a gesture, a menu or the palette was in flight.
    Cancels,
    /// Pressed with nothing in flight.
    Deselects,
}

/// The graph canvas before its geometry is settled: the graph, plus what this
/// frame's input means on it.
///
/// The prepass level of the context chain, between the [`GraphCtx`] and the
/// [`CanvasCtx`](crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx), which adds the geometry. The
/// two prepass steps that run *before*
/// [`CanvasGeometry::rebuild`](crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry::rebuild)
/// — pan/zoom and the node-drag advance — take this, so they read the frame's input without a
/// geometry someone is still writing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PrepassCtx<'a> {
    graph_ctx: GraphCtx<'a>,
    input: CanvasInput,
}

impl<'a> PrepassCtx<'a> {
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
        !matches!(self.input.escape, Escape::Unpressed)
    }

    /// The same frame with no gesture latched — see
    /// [`CanvasCtx::without_gesture`](crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx::without_gesture).
    pub(super) const fn without_gesture(self) -> Self {
        Self {
            input: CanvasInput {
                gesture: None,
                ..self.input
            },
            ..self
        }
    }
}
