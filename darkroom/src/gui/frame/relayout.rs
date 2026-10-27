//! Whether a frame owes palantir's layout engine a second pass.

use std::ops::{BitOr, BitOrAssign};

use crate::core::edit::edit_facts::EditFacts;

/// Whether anything a pass did stranded the canvas's cross-frame geometry
/// caches, so the frame owes a relayout.
///
/// Named rather than a `bool` because this travels a long way: every phase of the frontend's frame
/// reports one upward, each document mutation produces one, and the frontend spends the total on a
/// single relayout request. At a `-> bool` signature there is nothing to say whether `true` means
/// "did something", "succeeded", or "needs a pass" — and neighbouring methods on the same types
/// already return the first two.
///
/// [`Default`] is [`NotNeeded`](Self::NotNeeded), so a pass that reports
/// nothing costs the frame nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Relayout {
    /// Nothing the layout engine measures changed shape; this frame's cascade
    /// still describes the tree.
    #[default]
    NotNeeded,
    /// Something remeasured — a node's ports moved, a canvas appeared, a title
    /// grew — so the cached geometry read next frame would be stale.
    Needed,
}

impl Relayout {
    /// What an edit owes the canvas's cross-frame geometry caches. A node that came has no cached
    /// port offsets at all, and a rename or a const editor shown or hidden changes a measured
    /// size; a move or a reorder changes neither, as a port center resolves as `node.pos +
    /// cached offset`. Getting a move wrong is expensive rather than incorrect: a node drag emits
    /// one step per gesture frame, so a spurious relayout doubles the whole editor pipeline for
    /// the length of the drag.
    pub(crate) const fn after(facts: EditFacts) -> Self {
        Self::needed_if(facts.node_set || facts.renamed || facts.binding_kind)
    }

    /// Lift a predicate that already answers the question, for the passes that
    /// derive it from something else, as a canvas noticing it just came back on screen.
    pub(crate) const fn needed_if(stranded: bool) -> Self {
        if stranded {
            Self::Needed
        } else {
            Self::NotNeeded
        }
    }
}

impl BitOr for Relayout {
    type Output = Self;

    /// Accumulating: one pass needing a relayout is enough for the frame,
    /// however many others reported nothing.
    fn bitor(self, rhs: Self) -> Self {
        Self::needed_if(self == Self::Needed || rhs == Self::Needed)
    }
}

impl BitOrAssign for Relayout {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}

#[cfg(test)]
mod tests {
    use crate::core::edit::edit_facts::EditFacts;
    use crate::gui::frame::relayout::Relayout;

    /// Each fact alone costs the canvas a relayout, and an edit with none, a move, costs none.
    #[test]
    fn each_shape_fact_owes_a_relayout() {
        for (facts, relayout) in [
            (EditFacts::NONE, Relayout::NotNeeded),
            (
                EditFacts {
                    node_set: true,
                    ..EditFacts::NONE
                },
                Relayout::Needed,
            ),
            (
                EditFacts {
                    renamed: true,
                    ..EditFacts::NONE
                },
                Relayout::Needed,
            ),
            (
                EditFacts {
                    binding_kind: true,
                    ..EditFacts::NONE
                },
                Relayout::Needed,
            ),
        ] {
            assert_eq!(Relayout::after(facts), relayout, "{facts:?}");
        }
    }
}
