//! The reversible primitives an edit lands as, and the history stores.
//!
//! An [`UndoStep`](undo_step::UndoStep) is one slot of the document plus both
//! halves of what went into it — see [`Change`](change::Change) — so replaying
//! it in either [`Direction`](change::Direction) is the same write with the
//! other half. That pairing is what makes an inconsistent step
//! unconstructible: there is no separate snapshot to keep aligned with a
//! payload.
//!
//! Each kind is one file, holding its payload and its
//! [`Reversible`](reversible::Reversible) impl — how it writes, what it costs
//! the frame, whether it merges with the step before it. The trait is the
//! checklist: a kind that leaves a question unanswered does not compile, and
//! [`UndoStep`](undo_step::UndoStep) itself is nothing but a variant per kind
//! and one match that hands out the payload behind it.

pub(crate) mod change;
pub(crate) mod gesture_key;
pub(crate) mod move_selection;
pub(crate) mod node_presence;
pub(crate) mod raise;
pub(crate) mod rename_node;
pub(crate) mod reversible;
pub(crate) mod set_input;
pub(crate) mod set_node_property;
pub(crate) mod set_selection;
pub(crate) mod set_subscription;
pub(crate) mod set_viewport;
pub(crate) mod undo_step;

#[cfg(test)]
mod tests;
