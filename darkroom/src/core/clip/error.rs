//! Why a clip does not paste.

use scenarium::GraphSetValidationError;
use thiserror::Error;

use crate::core::edit::error::IntentError;

/// Why a clip does not paste into a document: its text came from another build, or a hand, and
/// names what this document cannot hold.
#[derive(Debug, Error)]
pub(crate) enum PasteError {
    /// An edit the paste lands that the edit gate does not take: a node whose func this library
    /// lacks, a wire to a port its declaration lacks, or one the document's rules refuse.
    #[error("the clip does not fit this graph")]
    Edit(#[source] IntentError),
    /// A subgraph the clip carries that breaks the load contract here, as one holding a node
    /// whose func this library lacks.
    #[error("a subgraph the clip carries does not hold here")]
    Subgraph(#[source] GraphSetValidationError),
}
