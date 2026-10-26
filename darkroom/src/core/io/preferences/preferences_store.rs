//! [`PreferencesStore`]: where a session's preferences persist.

use std::path::PathBuf;

use crate::core::io::preferences::error::PreferencesLoadError;
use crate::core::io::preferences::{self, Preferences};

/// Where a session reads its preferences from and writes its changes back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreferencesStore {
    /// A preferences file: read at launch, rewritten on each change.
    File(PathBuf),
    /// Nowhere: the session starts on the defaults and its changes stay in memory. A session whose
    /// file failed to load runs on this, so the file stays as the user has it; so does a harness
    /// session, which must change nothing on the machine.
    Memory,
}

impl PreferencesStore {
    /// The user's preferences file, in the platform's configuration directory.
    pub(crate) fn user() -> Self {
        Self::File(preferences::path())
    }

    /// The preferences this store holds: a missing file reads as the defaults, as on a first run.
    pub(crate) fn load(&self) -> Result<Preferences, PreferencesLoadError> {
        match self {
            Self::File(path) => Preferences::load_from(path),
            Self::Memory => Ok(Preferences::default()),
        }
    }
}
