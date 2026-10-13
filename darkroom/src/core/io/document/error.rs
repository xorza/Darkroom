//! Why a document archive could not be loaded or saved.

use std::io;
use std::path::PathBuf;

use common::{DeserializeError, SerializeError};
use zip::result::ZipError;

use crate::core::document::error::DocumentValidationError;
use crate::core::io::document::{DOCUMENT_ENTRY, EXTENSION, MAX_DOCUMENT_BYTES};

#[derive(Debug, thiserror::Error)]
pub(crate) enum DocumentLoadError {
    #[error("{path} must use the .{EXTENSION} extension", path = .path.display())]
    InvalidExtension { path: PathBuf },
    #[error("{path}: {source}", path = .path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not a valid Darkroom archive: {source}", path = .path.display())]
    InvalidArchive {
        path: PathBuf,
        #[source]
        source: ZipError,
    },
    #[error("failed to inspect {path}: {source}", path = .path.display())]
    InspectArchive {
        path: PathBuf,
        #[source]
        source: ZipError,
    },
    #[error("{path} contains overlapping ZIP entries", path = .path.display())]
    OverlappingEntries { path: PathBuf },
    #[error("{path} must contain exactly one {DOCUMENT_ENTRY}, found {count}", path = .path.display())]
    DocumentEntryCount { path: PathBuf, count: usize },
    #[error("failed to open {DOCUMENT_ENTRY} in {path}: {source}", path = .path.display())]
    OpenDocumentEntry {
        path: PathBuf,
        #[source]
        source: ZipError,
    },
    #[error("{path} contains a non-file {DOCUMENT_ENTRY} entry", path = .path.display())]
    NonFileDocumentEntry { path: PathBuf },
    #[error(
        "{DOCUMENT_ENTRY} in {path} is {size} bytes, exceeding the {max_mib} MiB size limit",
        path = .path.display(),
        max_mib = MAX_DOCUMENT_BYTES / (1024 * 1024)
    )]
    DocumentTooLarge { path: PathBuf, size: u64 },
    #[error("failed to read {DOCUMENT_ENTRY} from {path}: {source}", path = .path.display())]
    ReadDocument {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid {DOCUMENT_ENTRY} in {path}: {source}", path = .path.display())]
    DeserializeDocument {
        path: PathBuf,
        /// Boxed: a spanned RON error carries its whole span, which every
        /// `load` on the happy path would otherwise pay for in its `Result`.
        #[source]
        source: Box<DeserializeError>,
    },
    #[error("{path}: {source}", path = .path.display())]
    InvalidDocument {
        path: PathBuf,
        #[source]
        source: DocumentValidationError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DocumentSaveError {
    #[error("{path} must use the .{EXTENSION} extension", path = .path.display())]
    InvalidExtension { path: PathBuf },
    #[error("failed to serialize {DOCUMENT_ENTRY} for {path}: {source}", path = .path.display())]
    Serialize {
        path: PathBuf,
        #[source]
        source: SerializeError,
    },
    #[error(
        "{DOCUMENT_ENTRY} for {path} is {size} bytes, exceeding the {max_mib} MiB size limit",
        path = .path.display(),
        max_mib = MAX_DOCUMENT_BYTES / (1024 * 1024)
    )]
    DocumentTooLarge { path: PathBuf, size: u64 },
    /// Refused *before* writing, so a document that the next
    /// [`load`](crate::core::io::document::load) would reject can never
    /// replace the file already on disk. Same predicate as
    /// [`DocumentLoadError::InvalidDocument`] — the two directions can't
    /// drift.
    #[error("{path}: {source}", path = .path.display())]
    InvalidDocument {
        path: PathBuf,
        #[source]
        source: DocumentValidationError,
    },
    #[error("{path}: {source}", path = .path.display())]
    Publish {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
