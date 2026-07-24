//! Forward-only descriptions of graph mutations + the self-contained undo
//! entries built from them. The Intent/UndoStep model itself is documented
//! on [`types`].
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
//!   - [`query`] — the five exhaustive per-step predicates (`is_noop`,
//!     `requires_relayout`, `dirties_document`, `gesture_key`,
//!     `coalesce`) that drive the undo stack and the per-frame pipeline.
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
