//! The narrower context the canvas's *drawing* half records against.

use crate::core::document::item_id::ItemId;

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph_ui::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph_ui::draw::inspectors::Inspectors;
use crate::gui::pane::graph_ui::frame::canvas_geometry::CanvasGeometry;
use crate::gui::pane::graph_ui::frame::cull_region::CullRegion;
use crate::gui::pane::graph_ui::selection::Selection;
use crate::gui::theme::Theme;

/// Read-only context threaded top to bottom through everything the graph
/// canvas records. `Copy` (a canvas context plus its [`PaintFacts`]), so it's
/// passed by value — copying it while a borrow of the scene's node pool is
/// live is fine, which keeps `draw_all`'s node loop borrow-clean. The mutable
/// sinks, the request queue and the breaker probe, stay separate params.
///
/// The draw level of the context chain: derived from the [`CanvasCtx`], and
/// answering everything that one does — theme, geometry, graph — so the node
/// subtree names no other context. What it adds is what only the *paint* pass
/// knows: which nodes read as selected this frame, which panels are open, and
/// what the viewport keeps. The canvas-level draws that sit in the same pass
/// and want the same refs — the inspection panels
/// ([`crate::gui::pane::graph_ui::draw::inspectors`]) — take it too, rather than
/// each growing its own near-identical bundle.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawCtx<'a> {
    /// The canvas this record pass is drawing, and how the pass reaches the
    /// theme, the geometry, the graph and its library and run: all of it is
    /// already inside, so holding any of it again beside this would be two
    /// paths to one ref.
    canvas: CanvasCtx<'a>,
    facts: PaintFacts<'a>,
}

/// What only the paint pass knows: which nodes read as selected this frame,
/// which panels are open, and what the viewport keeps.
#[derive(Clone, Copy, Debug)]
pub(super) struct PaintFacts<'a> {
    /// Effective selection to paint: the graph's committed set or,
    /// mid-rubber-band, the live sweep — one type, so the draw substitutes
    /// them without caring which it got, and the gesture never writes its
    /// preview into the document.
    pub(super) selected: Selection<'a>,
    /// Open inspection panels, so the header chip can render its
    /// open/pinned state.
    pub(super) inspectors: &'a Inspectors,
    /// What the viewport keeps. Carried here rather than passed beside the
    /// context because every reader of one is a reader of the
    /// other: a pass that records nodes decides per node whether to.
    pub(super) cull: CullRegion,
}

impl<'a> DrawCtx<'a> {
    pub(super) const fn new(canvas: CanvasCtx<'a>, facts: PaintFacts<'a>) -> Self {
        Self { canvas, facts }
    }

    /// The palette and metrics this pass paints from, off the pane's context.
    pub(crate) const fn theme(self) -> &'a Theme {
        self.canvas.theme()
    }

    pub(crate) const fn graph_ctx(self) -> GraphCtx<'a> {
        self.canvas.graph_ctx()
    }

    pub(crate) const fn geometry(self) -> &'a CanvasGeometry {
        self.canvas.geometry()
    }

    pub(crate) const fn inspectors(self) -> &'a Inspectors {
        self.facts.inspectors
    }

    pub(crate) const fn cull(self) -> CullRegion {
        self.facts.cull
    }

    /// Whether `item` paints selected this pass.
    pub(crate) fn is_selected(self, item: ItemId) -> bool {
        self.facts.selected.contains(item)
    }
}
