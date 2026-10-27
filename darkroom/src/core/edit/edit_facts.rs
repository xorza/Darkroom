//! [`EditFacts`]: what a landed edit changed of the document's shape.

use std::ops::{BitOr, BitOrAssign};

/// What an edit, its undo or its redo changed that a frontend lays its view out by: facts about
/// the document, which a frontend maps onto its own caches. A move, a reorder or a selection
/// changes none of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EditFacts {
    /// A node or a stub came or went.
    pub(crate) node_set: bool,
    /// A node's name changed.
    pub(crate) renamed: bool,
    /// An input changed between a const and a wire or nothing.
    pub(crate) binding_kind: bool,
}

impl EditFacts {
    pub(crate) const NONE: Self = Self {
        node_set: false,
        renamed: false,
        binding_kind: false,
    };
}

impl BitOr for EditFacts {
    type Output = Self;

    /// What either batch changed.
    fn bitor(self, rhs: Self) -> Self {
        Self {
            node_set: self.node_set || rhs.node_set,
            renamed: self.renamed || rhs.renamed,
            binding_kind: self.binding_kind || rhs.binding_kind,
        }
    }
}

impl BitOrAssign for EditFacts {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}
