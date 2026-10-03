//! Why the preferences file could not be read.

use std::io;
use std::path::PathBuf;

use common::DeserializeError;

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
