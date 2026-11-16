//! Why a clip does not paste.

use thiserror::Error;

use crate::core::edit::error::IntentError;

/// Why a clip does not paste into a document: its text came from another build, or a hand, and
/// lands an edit the edit gate does not take: a node whose func this library lacks, a wire to a
/// port its declaration lacks, or one the document's rules refuse.
#[derive(Debug, Error)]
#[error("the clip does not fit this graph")]
pub(crate) struct PasteError(#[source] pub(crate) IntentError);
