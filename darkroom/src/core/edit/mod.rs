//! Editing the document's graph, from a widget's request to undoable history: the intents every
//! surface raises ([`graph_intent`]), the gate that validates them, the reversible steps they
//! land as ([`step`]), and the packed history those steps are stored in ([`undo_history`]).

pub(crate) mod document_queue;
pub(crate) mod document_request;
pub(crate) mod edit_facts;
pub(crate) mod error;
pub(crate) mod gesture_id;
pub(crate) mod graph_intent;
pub(crate) mod step;
pub(crate) mod undo_history;
mod unwired;
mod validate;
