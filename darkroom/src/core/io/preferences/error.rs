//! Why the preferences file could not be read or written.

use std::io;
use std::path::PathBuf;

use common::{DeserializeError, SerializeError};

/// The preferences file exists but could not be read. A missing file is not
/// one: it reads as the defaults.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PreferencesLoadError {
    #[error("could not read preferences '{path}': {source}", path = .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse preferences '{path}': {source}", path = .path.display())]
    Parse {
        path: PathBuf,
        /// Boxed: a RON parse error is several times the size of the rest.
        #[source]
        source: Box<DeserializeError>,
    },
}

/// The preferences could not be written. The file on disk is left as it was.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PreferencesSaveError {
    #[error("could not serialize preferences: {0}")]
    Serialize(#[from] SerializeError),
    #[error("could not create the configuration directory '{path}': {source}", path = .path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not write preferences '{path}': {source}", path = .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
