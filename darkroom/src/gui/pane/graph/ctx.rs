//! The canvas's two contexts: one graph pane for the whole frame, and the
//! narrower one its *drawing* half records against.

use std::collections::BTreeSet;

use scenarium::NodeId;

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::frame::cull::CullRegion;
use crate::gui::pane::graph::frame::geometry::CanvasGeometry;
use crate::gui::pane::graph::gesture::canvas_gesture::CanvasGesture;
use crate::gui::pane::graph::paint::inspector::Inspectors;
use crate::gui::theme::Theme;

/// One graph pane's canvas for this frame: the pane itself, plus the four
/// facts [`GraphUI::prepass`](crate::gui::pane::graph::GraphUI::prepass) resolves
/// once and every controller then reads —
/// last frame's port geometry, this frame's swept node hits, the bare-canvas
/// gesture that latched, and whether Esc cancelled it.
///
/// The canvas level of the context chain, derived from the pane's
/// [`GraphCtx`] and answering everything that one does. Before it, each
/// controller took those four as parameters and the compiler had no way to
/// say they all described the same frame.
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
    pub(super) fn new(
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

    pub(crate) fn graph_ctx(self) -> GraphCtx<'a> {
        self.graph_ctx
    }

    pub(crate) fn theme(self) -> &'a Theme {
        self.graph_ctx.theme()
    }

    /// Last frame's port centers and node rects.
    pub(crate) fn geometry(self) -> &'a CanvasGeometry {
        self.geometry
    }

    /// Which bare-canvas gesture latched this frame, if any. Canvas-private:
    /// the classification is this module's arbitration, and no reader outside
    /// it has a use for the answer.
    pub(super) fn gesture(self) -> Option<CanvasGesture> {
        self.gesture
    }

    /// Whether this frame's Esc cancels whatever gesture is in flight.
    pub(super) fn cancelled(self) -> bool {
        self.cancelled
    }

    /// The same canvas with no gesture latched — for the one reader that has
    /// to be told this frame's gesture is not for it (a right-click that just
    /// ended a floating wire must not also open the palette). A derived
    /// context rather than a `gesture` parameter beside this one, so there is
    /// still exactly one answer in scope at the call site.
    pub(super) fn without_gesture(self) -> Self {
        Self {
            gesture: None,
            ..self
        }
    }
}

/// Read-only context threaded top to bottom through everything one graph
/// pane records. `Copy` (a canvas context plus three shared refs), so it's
/// passed by value — copying it while a borrow of the scene's node pool is
/// live is fine, which keeps `draw_all`'s node loop borrow-clean. The mutable
/// sinks (`out`, `actions`) and the breaker `probe` stay separate params.
///
/// The draw level of the context chain: derived from the pane's
/// [`CanvasCtx`], and answering everything that one does — theme, geometry,
/// hits, the pane itself — so the node subtree names no other context. What
/// it adds is what only the *paint* pass knows: which nodes read as selected
/// this frame, which panels are open, and what the viewport keeps. The
/// canvas-level draws that sit in the same pass and want the same refs — the
/// inspection panels ([`crate::gui::pane::graph::paint::inspector`]) — take it too,
/// rather than each growing its own near-identical bundle.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawCtx<'a> {
    /// The one canvas this record pass is drawing, and how the pass reaches
    /// the theme, the geometry, the pane and its library and run: all of it
    /// is already inside, so holding any of it again beside this would be two
    /// paths to one ref. Every other pane on screen gets its own `DrawCtx`,
    /// so nothing here can reach across.
    canvas: CanvasCtx<'a>,
    /// Effective selection to paint: the graph's committed set or,
    /// mid-rubber-band, the live sweep — one type, so the draw substitutes
    /// them without caring which it got, and the gesture never writes its
    /// preview into the document.
    selected: Selection<'a>,
    /// Open inspection panels, so the header chip can render its
    /// open/pinned state.
    inspectors: &'a Inspectors,
    /// What this pane's viewport keeps. Carried here rather than passed
    /// beside the context because every reader of one is a reader of the
    /// other: a pass that records nodes decides per node whether to.
    cull: CullRegion,
}

impl<'a> DrawCtx<'a> {
    pub(super) fn new(
        canvas: CanvasCtx<'a>,
        selected: Selection<'a>,
        inspectors: &'a Inspectors,
        cull: CullRegion,
    ) -> Self {
        Self {
            canvas,
            selected,
            inspectors,
            cull,
        }
    }

    /// The palette and metrics this pass paints from, off the pane's context.
    pub(crate) fn theme(self) -> &'a Theme {
        self.canvas.theme()
    }

    pub(crate) fn graph_ctx(self) -> GraphCtx<'a> {
        self.canvas.graph_ctx()
    }

    pub(crate) fn geometry(self) -> &'a CanvasGeometry {
        self.canvas.geometry()
    }

    pub(crate) fn inspectors(self) -> &'a Inspectors {
        self.inspectors
    }

    pub(crate) fn cull(self) -> CullRegion {
        self.cull
    }

    /// Whether `key` paints selected this pass.
    pub(crate) fn is_selected(self, key: NodeId) -> bool {
        self.selected.contains(key)
    }
}

/// What reads as selected while a pane records: the document's committed set,
/// or a rubber band's live sweep.
///
/// Two representations because the two halves want different things. The
/// document keeps a `BTreeSet` — it is persisted, diffed by undo, and handed
/// to `GraphIntent::SetSelection`. The sweep is rebuilt from scratch *every
/// frame* a band is held, so it wants a buffer whose `clear` keeps its
/// capacity, which a `BTreeSet` does not have (`clear` drops the whole tree).
/// A sorted slice gives the sweep that and still answers the only question the
/// draw asks — [`Self::contains`], once per node per frame — in log time.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Selection<'a> {
    /// Straight off the graph's view.
    Committed(&'a BTreeSet<NodeId>),
    /// A band's live sweep. **Sorted and deduplicated** — build it through
    /// [`Self::swept`], which is where that is checked.
    Swept(&'a [NodeId]),
}

impl<'a> Selection<'a> {
    /// The sweep's view of `sorted`.
    ///
    /// The precondition is checked here rather than trusted: [`Self::contains`]
    /// binary-searches, so an unsorted slice answers *wrong* rather than
    /// merely slowly — a node would silently stop painting selected. Debug
    /// only, and once per pane per frame rather than per node, so the release
    /// build pays nothing for it.
    pub(crate) fn swept(sorted: &'a [NodeId]) -> Self {
        debug_assert!(
            sorted.windows(2).all(|w| w[0] < w[1]),
            "a swept selection must be sorted and deduplicated: {sorted:?}"
        );
        Self::Swept(sorted)
    }

    pub(crate) fn contains(self, id: NodeId) -> bool {
        match self {
            Self::Committed(set) => set.contains(&id),
            Self::Swept(sorted) => sorted.binary_search(&id).is_ok(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two representations must answer `contains` identically — the draw
    /// substitutes one for the other mid-gesture and can't be told which it
    /// got, so a disagreement would show up as nodes changing selected-ness
    /// when a band starts rather than when it sweeps them.
    #[test]
    fn both_representations_agree_on_membership() {
        let ids: Vec<NodeId> = (0..6).map(|_| NodeId::unique()).collect();
        // Members: three of the six, in the sweep's sorted order.
        let mut members: Vec<NodeId> = vec![ids[0], ids[2], ids[4]];
        members.sort_unstable();
        let set: BTreeSet<NodeId> = members.iter().copied().collect();

        let committed = Selection::Committed(&set);
        let swept = Selection::swept(&members);
        for id in &ids {
            assert_eq!(
                committed.contains(*id),
                swept.contains(*id),
                "the two views disagree about {id:?}"
            );
        }
        // And they agree about *which* three, not merely about the count.
        for id in &members {
            assert!(swept.contains(*id), "{id:?} was swept");
        }
        assert_eq!(
            ids.iter().filter(|id| swept.contains(**id)).count(),
            3,
            "exactly the three members, and nothing else"
        );
    }

    /// The empty sweep is the state between gestures, and it must answer
    /// `false` rather than panicking on the binary search.
    #[test]
    fn an_empty_sweep_holds_nothing() {
        assert!(!Selection::swept(&[]).contains(NodeId::unique()));
    }
}
