//! Forward-only descriptions of graph mutations + the self-contained undo
//! entries built from them.
//!
//! An [`Intent`](types::Intent) is "what the caller wants the graph to look
//! like after"; it carries no history. To make the change reversible, we
//! pair the intent with a snapshot of the slot it overwrites. Rather than
//! carrying that snapshot in a sibling enum, [`UndoStep`](types::UndoStep)
//! folds both halves into one variant per kind: every variant has both the
//! "from" payload (for revert) and the "to" payload (for forward apply).
//! Type-level enforcement means an `UndoStep` can never be constructed
//! inconsistently — there's no `(Intent::A, Snapshot::B)` mismatch to worry
//! about at runtime.
//!
//! Split by responsibility:
//!   - [`types`] — the `Intent` / `UndoStep` / `GraphStep` / `DocStep` /
//!     `GestureKey` model.
//!   - [`build`] — `build_step`: read the pre-mutation snapshot from
//!     `&Document`, fold with the incoming intent, return a fully-populated
//!     `UndoStep`. Pure.
//!   - [`apply`] — `apply_step` / `revert_step` write the "to"/"from" half
//!     of an `UndoStep` to `&mut Document` (used by initial commit,
//!     undo-stack redo, and undo respectively), plus the high-level
//!     `commit_intent_cascading` entry the live frontends drive their
//!     per-intent loop through: build → no-op-filter → apply, then cascade
//!     the edits an input change implies (a wildcard-output retype drops
//!     the wires it invalidated).
//!   - [`query`] — the six exhaustive per-step predicates (`is_noop`,
//!     `requires_relayout`, `requires_reconcile`, `dirties_document`,
//!     `gesture_key`, `coalesce`) that drive the undo stack and the
//!     per-frame pipeline.
//!   - [`duplicate`] — editor-side `Intent::DuplicateNodes` construction
//!     from a selection (kept here rather than on `Document`, which is the
//!     persisted model — intent construction is editing machinery).

pub(crate) mod apply;
pub(crate) mod build;
pub(crate) mod duplicate;
pub(crate) mod query;
pub(crate) mod types;

#[cfg(test)]
mod tests;
