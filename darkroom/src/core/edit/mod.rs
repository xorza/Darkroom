//! Editing the document's graph, from a widget's request to undoable history.
//!
//! Three modules, in the order an edit travels them:
//!
//!   - [`graph_intent`] — the forward-only vocabulary every surface speaks,
//!     and *the* gate: one entry validates an intent against the live
//!     document, captures what it overwrites, and folds the two into a step.
//!     What it refuses ([`error`]) is our own bug; what it drops quietly is
//!     an ordinary stale gesture. The checks it is assembled from live in
//!     `validate`, split out so the trust boundary reads as one list rather
//!     than as asides inside the fold.
//!   - [`step`] — the reversible primitives, each carrying both halves of
//!     what it wrote so a replay in either direction is one call.
//!   - [`undo_history`] — the packed undo history those steps are stored in.

pub(crate) mod document_queue;
pub(crate) mod document_request;
pub(crate) mod error;
pub(crate) mod gesture_id;
pub(crate) mod graph_intent;
pub(crate) mod relayout;
pub(crate) mod step;
pub(crate) mod undo_history;
mod unwired;
mod validate;
