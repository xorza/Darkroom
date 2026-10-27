//! [`Selection`]: which items read as selected this frame, committed or swept.

use std::collections::BTreeSet;

use crate::core::document::item_id::ItemId;

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
    Committed(&'a BTreeSet<ItemId>),
    /// A band's live sweep. **Sorted and deduplicated** — build it through
    /// [`Self::swept`], which is where that is checked.
    Swept(&'a [ItemId]),
}

impl<'a> Selection<'a> {
    /// The sweep's view of `sorted`.
    ///
    /// The precondition is checked here rather than trusted: [`Self::contains`]
    /// binary-searches, so an unsorted slice answers *wrong* rather than
    /// merely slowly — an item would silently stop painting selected. Debug
    /// only, and once per frame rather than per node, so the release build
    /// pays nothing for it.
    pub(crate) fn swept(sorted: &'a [ItemId]) -> Self {
        debug_assert!(
            sorted.windows(2).all(|w| w[0] < w[1]),
            "a swept selection must be sorted and deduplicated: {sorted:?}"
        );
        Self::Swept(sorted)
    }

    pub(crate) fn contains(self, item: ItemId) -> bool {
        match self {
            Self::Committed(set) => set.contains(&item),
            Self::Swept(sorted) => sorted.binary_search(&item).is_ok(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::NodeId;

    use crate::core::document::tie::TieId;

    /// The two representations must answer `contains` identically — the draw
    /// substitutes one for the other mid-gesture and can't be told which it
    /// got, so a disagreement would show up as nodes changing selected-ness
    /// when a band starts rather than when it sweeps them. Nodes and ties
    /// mix, as a band sweeps both.
    #[test]
    fn both_representations_agree_on_membership() {
        let ids: Vec<ItemId> = (0..6)
            .map(|i| {
                if i < 3 {
                    ItemId::Node(NodeId::unique())
                } else {
                    ItemId::Tie(TieId::unique())
                }
            })
            .collect();
        // Members: three of the six, in the sweep's sorted order.
        let mut members: Vec<ItemId> = vec![ids[0], ids[2], ids[4]];
        members.sort_unstable();
        let set: BTreeSet<ItemId> = members.iter().copied().collect();

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
        assert!(!Selection::swept(&[]).contains(ItemId::Node(NodeId::unique())));
    }
}
