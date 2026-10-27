//! The reversible primitives an edit lands as, and the history stores.
//!
//! An [`UndoStep`](undo_step::UndoStep) is one slot of the document plus what
//! a replay in either [`Direction`](change::Direction) writes there: both
//! halves of a [`Change`](change::Change), a node's whole entry and which way
//! it moves, or, for a change that is its own inverse, the change alone, as a
//! selection's toggles. Each shape admits only the steps it means: there is no
//! separate snapshot to keep aligned with a payload.
//!
//! Each kind is one file, holding its payload and its
//! [`Reversible`](reversible::Reversible) impl — how it writes and what it
//! costs the frame. The trait is the checklist: a kind that leaves a question
//! unanswered does not compile, and [`UndoStep`](undo_step::UndoStep) itself
//! is a variant per kind, one match that hands out the payload behind it, and
//! the fold a held gesture's frames take.

pub(crate) mod change;
pub(crate) mod reversible;
pub(crate) mod set_depth;
pub(crate) mod set_input;
pub(crate) mod set_node;
pub(crate) mod set_node_name;
pub(crate) mod set_node_property;
pub(crate) mod set_positions;
pub(crate) mod set_selection;
pub(crate) mod set_subscription;
pub(crate) mod set_ties;
pub(crate) mod undo_step;

#[cfg(test)]
mod tests;
