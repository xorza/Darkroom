//! Forward-only descriptions of graph mutations + the self-contained undo
//! entries built from them. The Intent/UndoStep model itself is documented
//! on [`types`].
//!
//! Split by responsibility:
//!   - [`types`] — the `Intent` / `UndoStep` / `UndoStep` / `DockStep` /
//!     `GestureKey` model.
//!   - [`build`] — `build_step` / `build_doc_step`: read the pre-mutation
//!     snapshot from `&Document`, fold with the incoming intent, return a
//!     fully-populated step. Pure. Also *the* validation gate: it's the
//!     single entry every frontend commits through, so each arm establishes
//!     the whole precondition set its `apply` half assumes, and refuses
//!     anything else with a `Refusal`.
//!   - [`validate`] — the checks those arms are built from (is this id
//!     resolvable, fresh, non-nil; is this position finite; is this kind
//!     insertable here), split out so the trust boundary reads as one
//!     list rather than as asides inside `build_step`.
//!   - [`apply`] — `apply_step` / `revert_step` write the "to"/"from" half
//!     of an `UndoStep` to `&mut Document` (used by initial commit,
//!     undo-stack redo, and undo respectively), plus the `commit_intent` /
//!     `commit_dock_op` entries the live frontends drive their
//!     per-intent loop through: build → no-op-filter → apply. Uniform
//!     across variants — a wildcard-output retype severs nothing
//!     (mismatched wires are tolerated and flatten as unbound).
//!   - [`query`] — the five exhaustive per-step predicates (`is_noop`,
//!     `invalidates_cached_geometry`, `dirties_document`, `gesture_key`,
//!     `coalesce`) that drive the undo stack and the per-frame pipeline.
//!   - [`duplicate`] — editor-side `Intent::DuplicateNodes` construction
//!     from a selection (kept here rather than on `Document`, which is the
//!     persisted model — intent construction is editing machinery).
//!   - [`sink`] — `Intents`, the frame's queue: an `Intent` paired with the
//!     graph it commits against, or a bare `DockOp`. Several graph panes
//!     can be on screen, so a target travels with every scoped intent rather
//!     than being resolved once per frame.

pub(crate) mod apply;
pub(crate) mod build;
pub(crate) mod duplicate;
mod query;
pub(crate) mod sink;
pub(crate) mod types;
mod validate;

#[cfg(test)]
mod tests;
