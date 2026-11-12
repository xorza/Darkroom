//! The narrower context the canvas's *drawing* half records against.

use crate::core::document::item_id::ItemId;

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::scene::canvas_geometry::CanvasGeometry;
use crate::gui::pane::graph_ui::scene::cull_region::CullRegion;
use crate::gui::pane::graph_ui::selection::Selection;
use crate::gui::theme::Theme;

/// Read-only context threaded top to bottom through everything the graph
/// canvas records. `Copy`, so it's passed by value: copying it while a borrow of the scene's node
/// pool is live is fine, which keeps `draw_all`'s node loop borrow-clean. The mutable sinks, the
/// request queue and the breaker probe, stay separate params.
///
/// The draw level of the context chain, so the node subtree names no other context. What it adds is
/// what only the *paint* pass knows: which nodes read as selected this frame, and what the viewport
/// keeps.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawCtx<'a> {
    pub(super) graph_ctx: GraphCtx<'a>,
    pub(super) geometry: &'a CanvasGeometry,
    /// The selection to paint: the graph's committed set or, mid-rubber-band, the live sweep,
    /// so the gesture never writes its preview into the document.
    pub(super) selected: Selection<'a>,
    /// What the viewport keeps; every reader of the context decides per node whether to record.
    pub(super) cull: CullRegion,
}

impl<'a> DrawCtx<'a> {
    /// The palette and metrics this pass paints from, off the pane's context.
    pub(crate) const fn theme(self) -> &'a Theme {
        self.graph_ctx.theme()
    }

    pub(crate) const fn graph_ctx(self) -> GraphCtx<'a> {
        self.graph_ctx
    }

    pub(crate) const fn geometry(self) -> &'a CanvasGeometry {
        self.geometry
    }

    pub(crate) const fn cull(self) -> CullRegion {
        self.cull
    }

    /// The zoom this pass paints at: a screen-sized stroke divides by it.
    pub(crate) const fn zoom(self) -> f32 {
        self.graph_ctx.camera().zoom
    }

    /// Whether `item` paints selected this pass.
    pub(crate) fn is_selected(self, item: ItemId) -> bool {
        self.selected.contains(item)
    }
}
