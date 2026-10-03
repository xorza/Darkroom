//! The narrower context the canvas's *drawing* half records against, and the
//! selection it paints.

use std::collections::BTreeSet;

use scenarium::NodeId;

use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::canvas_ctx::CanvasCtx;
use crate::gui::pane::graph::frame::cull::CullRegion;
use crate::gui::pane::graph::frame::geometry::CanvasGeometry;
use crate::gui::pane::graph::paint::inspector::Inspectors;
use crate::gui::theme::Theme;

/// Read-only context threaded top to bottom through everything the graph
/// canvas records. `Copy` (a canvas context plus three shared refs), so it's
/// passed by value — copying it while a borrow of the scene's node pool is
/// live is fine, which keeps `draw_all`'s node loop borrow-clean. The mutable
/// sinks (`out`, `actions`) and the breaker `probe` stay separate params.
///
/// The draw level of the context chain: derived from the [`CanvasCtx`], and
/// answering everything that one does — theme, geometry, graph — so the node
/// subtree names no other context. What it adds is what only the *paint* pass
/// knows: which nodes read as selected this frame, which panels are open, and
/// what the viewport keeps. The canvas-level draws that sit in the same pass
/// and want the same refs — the inspection panels
/// ([`crate::gui::pane::graph::paint::inspector`]) — take it too, rather than
/// each growing its own near-identical bundle.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawCtx<'a> {
    /// The canvas this record pass is drawing, and how the pass reaches the
    /// theme, the geometry, the graph and its library and run: all of it is
    /// already inside, so holding any of it again beside this would be two
    /// paths to one ref.
    canvas: CanvasCtx<'a>,
    /// Effective selection to paint: the graph's committed set or,
    /// mid-rubber-band, the live sweep — one type, so the draw substitutes
    /// them without caring which it got, and the gesture never writes its
    /// preview into the document.
    selected: Selection<'a>,
    /// Open inspection panels, so the header chip can render its
    /// open/pinned state.
    inspectors: &'a Inspectors,
    /// What the viewport keeps. Carried here rather than passed beside the
    /// context because every reader of one is a reader of the
    /// other: a pass that records nodes decides per node whether to.
    cull: CullRegion,
}

impl<'a> DrawCtx<'a> {
    pub(super) const fn new(
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
        self.inspectors
    }

    pub(crate) const fn cull(self) -> CullRegion {
        self.cull
    }

    /// Whether `key` paints selected this pass.
    pub(crate) fn is_selected(self, key: NodeId) -> bool {
        self.selected.contains(key)
    }
}

/// What reads as selected while the canvas records: the document's committed
/// set, or a rubber band's live sweep.
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
    /// only, and once per frame rather than per node, so the release build
    /// pays nothing for it.
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
