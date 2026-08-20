//! The two halves of a reversible write, and which of them a replay puts back.

use serde::{Deserialize, Serialize};

/// What a slot held before an edit and what the edit leaves in it.
///
/// Every [`UndoStep`](crate::core::edit::step::undo_step::UndoStep) is an
/// anchor plus one or more of these, and writing a step is picking the half a
/// [`Direction`] names. Holding both halves in one value is what makes a
/// mismatched step unconstructible: there is no sibling snapshot that could
/// drift out of step with the payload it describes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Change<T> {
    /// What the slot held before — what a revert puts back.
    pub(crate) from: T,
    /// What the edit leaves in the slot — what an apply writes.
    pub(crate) to: T,
}

impl<T> Change<T> {
    /// The half `dir` writes.
    pub(super) fn half(&self, dir: Direction) -> &T {
        match dir {
            Direction::Forward => &self.to,
            Direction::Backward => &self.from,
        }
    }
}

impl<T: PartialEq> Change<T> {
    /// Whether the two halves are the same value, so writing either one
    /// leaves the document as it was. What
    /// [`Reversible::is_noop`](crate::core::edit::step::reversible::Reversible::is_noop)
    /// is for all but the kinds that compare with a tolerance.
    pub(super) fn unchanged(&self) -> bool {
        self.from == self.to
    }
}

/// Which half of a [`Change`] a replay writes into the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    /// Write the `to` half: the initial commit, and undo-stack redo.
    Forward,
    /// Write the `from` half: undo.
    Backward,
}

impl Direction {
    /// The direction that undoes this one — so
    /// [`Change::half`] of it is the half being overwritten, which is what a
    /// destructive write checks itself against.
    pub(super) fn reversed(self) -> Self {
        match self {
            Self::Forward => Self::Backward,
            Self::Backward => Self::Forward,
        }
    }
}
