//! The canvas's frame-wide context: the graph plus the three facts the
//! prepass settles before any controller reads them.

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry;
use crate::gui::pane::graph_ui::prepass_ctx::{CanvasGesture, CanvasInput, Escape, PrepassCtx};

/// The graph canvas for this frame: the graph, plus the three facts
/// [`GraphUi::prepass`](crate::gui::pane::graph_ui::GraphUi::prepass) resolves
/// once and every controller then reads — the port and node geometry, the
/// bare-canvas gesture that latched, and whether Esc cancelled it.
///
/// The canvas level of the context chain, built from the [`PrepassCtx`], so the compiler holds the
/// three facts to one frame.
///
/// **It exists only once the geometry is settled.** The table is borrowed
/// shared for the context's whole life, so the three prepass steps that run
/// *before* [`CanvasGeometry::rebuild`] — pan/zoom, the group drag's advance and the tie knob's
/// turn — take the [`PrepassCtx`], and `bake_snap_hover` (which needs the table
/// `&mut` again) ends it. That ordering is the point rather than an
/// inconvenience: a controller holding one of these cannot be reading a
/// geometry someone is still writing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CanvasCtx<'a> {
    graph_ctx: GraphCtx<'a>,
    input: CanvasInput,
    geometry: &'a CanvasGeometry,
}

impl<'a> CanvasCtx<'a> {
    pub(super) const fn new(prepass: PrepassCtx<'a>, geometry: &'a CanvasGeometry) -> Self {
        Self {
            graph_ctx: prepass.graph_ctx(),
            input: prepass.input(),
            geometry,
        }
    }

    pub(crate) const fn graph_ctx(self) -> GraphCtx<'a> {
        self.graph_ctx
    }

    /// Last frame's port centers and node rects.
    pub(crate) const fn geometry(self) -> &'a CanvasGeometry {
        self.geometry
    }

    /// Which bare-canvas gesture latched this frame, if any. Canvas-private:
    /// the classification is this module's arbitration, and no reader outside
    /// it has a use for the answer.
    pub(super) const fn gesture(self) -> Option<CanvasGesture> {
        self.input.gesture
    }

    /// This context with the gesture a later read of the frame classified.
    pub(super) const fn with_gesture(mut self, gesture: Option<CanvasGesture>) -> Self {
        self.input.gesture = gesture;
        self
    }

    /// Whether this frame's Esc cancels whatever gesture is in flight.
    pub(super) const fn cancelled(self) -> bool {
        self.input.cancelled()
    }

    /// Whether this frame's Esc clears the selection: pressed with nothing in
    /// flight.
    pub(super) const fn deselects(self) -> bool {
        matches!(self.input.escape, Escape::Deselects)
    }
}
