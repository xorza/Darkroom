//! Why the preferences file could not be read or written.

use std::io;
use std::path::PathBuf;

use common::{DeserializeError, SerializeError};

/// The preferences file exists but could not be read or parsed. A missing file is not one: it
/// reads as the defaults.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PreferencesLoadError {
    #[error(transparent)]
    Read(#[from] PreferencesReadError),
    #[error(transparent)]
    Parse(#[from] PreferencesParseError),
}

/// The preferences file exists but could not be read.
#[derive(Debug, thiserror::Error)]
#[error("could not read preferences '{path}'", path = .path.display())]
pub(crate) struct PreferencesReadError {
    pub(crate) path: PathBuf,
    #[source]
    pub(crate) source: io::Error,
}

/// The preferences file was read but does not parse.
#[derive(Debug, thiserror::Error)]
#[error("could not parse preferences '{path}'", path = .path.display())]
pub(crate) struct PreferencesParseError {
    pub(crate) path: PathBuf,
    /// Boxed: a RON parse error is several times the size of the rest.
    #[source]
    pub(crate) source: Box<DeserializeError>,
}

/// A preferences file that did not parse could not be set aside, so it stays where it was.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PreferencesSetAsideError {
    #[error("every backup name for '{path}' is taken", path = .path.display())]
    Exhausted { path: PathBuf },
    #[error("could not set the preferences aside as '{path}'", path = .path.display())]
    Move {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// The preferences could not be written. The file on disk is left as it was.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PreferencesSaveError {
    #[error("could not serialize preferences")]
    Serialize(#[from] SerializeError),
    #[error("could not create the configuration directory '{path}'", path = .path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not write preferences '{path}'", path = .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
