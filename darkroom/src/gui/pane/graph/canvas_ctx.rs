//! The canvas's frame-wide context: the graph plus the three facts the
//! prepass settles before any controller reads them.

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::frame::geometry::CanvasGeometry;
use crate::gui::pane::graph::gesture::canvas_gesture::CanvasGesture;
use crate::gui::theme::Theme;

/// The graph canvas for this frame: the graph, plus the three facts
/// [`GraphUI::prepass`](crate::gui::pane::graph::GraphUI::prepass) resolves
/// once and every controller then reads — the port and node geometry, the
/// bare-canvas gesture that latched, and whether Esc cancelled it.
///
/// The canvas level of the context chain, derived from the [`GraphCtx`] and
/// answering everything that one does, so the compiler holds the three facts
/// to one frame.
///
/// **It exists only once the geometry is settled.** The table is borrowed
/// shared for the context's whole life, so the two prepass steps that run
/// *before* [`CanvasGeometry::rebuild`] — pan/zoom and the node-drag advance —
/// take the graph context directly, and `bake_snap_hover` (which needs the table
/// `&mut` again) ends it. That ordering is the point rather than an
/// inconvenience: a controller holding one of these cannot be reading a
/// geometry someone is still writing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CanvasCtx<'a> {
    graph_ctx: GraphCtx<'a>,
    geometry: &'a CanvasGeometry,
    gesture: Option<CanvasGesture>,
    cancelled: bool,
}

impl<'a> CanvasCtx<'a> {
    pub(super) const fn new(
        graph_ctx: GraphCtx<'a>,
        geometry: &'a CanvasGeometry,
        gesture: Option<CanvasGesture>,
        cancelled: bool,
    ) -> Self {
        Self {
            graph_ctx,
            geometry,
            gesture,
            cancelled,
        }
    }

    pub(crate) const fn graph_ctx(self) -> GraphCtx<'a> {
        self.graph_ctx
    }

    pub(crate) const fn theme(self) -> &'a Theme {
        self.graph_ctx.theme()
    }

    /// Last frame's port centers and node rects.
    pub(crate) const fn geometry(self) -> &'a CanvasGeometry {
        self.geometry
    }

    /// Which bare-canvas gesture latched this frame, if any. Canvas-private:
    /// the classification is this module's arbitration, and no reader outside
    /// it has a use for the answer.
    pub(super) const fn gesture(self) -> Option<CanvasGesture> {
        self.gesture
    }

    /// Whether this frame's Esc cancels whatever gesture is in flight.
    pub(super) const fn cancelled(self) -> bool {
        self.cancelled
    }

    /// The same canvas with no gesture latched — for the one reader that has
    /// to be told this frame's gesture is not for it (a right-click that just
    /// ended a floating wire must not also open the palette). A derived
    /// context rather than a `gesture` parameter beside this one, so there is
    /// still exactly one answer in scope at the call site.
    pub(super) const fn without_gesture(self) -> Self {
        Self {
            gesture: None,
            ..self
        }
    }
}
