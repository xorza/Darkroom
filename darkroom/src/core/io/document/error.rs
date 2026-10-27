//! Why a document file could not be loaded or saved.

use std::io;
use std::path::PathBuf;

use common::{DeserializeError, SerializeError};
use zip::result::ZipError;

use crate::core::io::document::document_format::DOCUMENT_ENTRY;
use crate::core::io::document::{EXTENSION, MAX_DOCUMENT_BYTES};

#[derive(Debug, thiserror::Error)]
pub(crate) enum DocumentLoadError {
    #[error("{path} must use the .{EXTENSION} extension", path = .path.display())]
    InvalidExtension { path: PathBuf },
    #[error(
        "{path} holds paths relative to its folder, whose name is not valid Unicode",
        path = .path.display()
    )]
    NonUnicodeFolder { path: PathBuf },
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
        "the document in {path} is {size} bytes, over the {max_mib} MiB size limit",
        path = .path.display(),
        max_mib = MAX_DOCUMENT_BYTES / (1024 * 1024)
    )]
    DocumentTooLarge { path: PathBuf, size: u64 },
    #[error("failed to read {path}: {source}", path = .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not a valid Darkroom document: {source}", path = .path.display())]
    Deserialize {
        path: PathBuf,
        /// Boxed: a spanned RON error carries its whole span, which every
        /// `load` on the happy path would otherwise pay for in its `Result`.
        #[source]
        source: Box<DeserializeError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DocumentSaveError {
    #[error("{path} must use the .{EXTENSION} extension", path = .path.display())]
    InvalidExtension { path: PathBuf },
    #[error("failed to serialize the document for {path}: {source}", path = .path.display())]
    Serialize {
        path: PathBuf,
        #[source]
        source: SerializeError,
    },
    #[error(
        "the document for {path} is {size} bytes, over the {max_mib} MiB size limit",
        path = .path.display(),
        max_mib = MAX_DOCUMENT_BYTES / (1024 * 1024)
    )]
    DocumentTooLarge { path: PathBuf, size: u64 },
    #[error("{path}: {source}", path = .path.display())]
    Publish {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write the archive {path}: {source}", path = .path.display())]
    Archive {
        path: PathBuf,
        #[source]
        source: ZipError,
    },
}

/// Why writing a document's bytes into its file failed, before the save names the file.
#[derive(Debug, thiserror::Error)]
pub(super) enum DocumentWriteError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Archive(#[from] ZipError),
}
