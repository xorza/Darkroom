//! [`PreferencesStore`]: where a session's preferences persist.

use std::path::PathBuf;

use crate::core::io::preferences::error::{PreferencesLoadError, PreferencesSaveError};
use crate::core::io::preferences::{PREFERENCES_FILE, Preferences};
use crate::platform;

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

/// What a session opens its preferences as: the preferences, the store their changes persist
/// to, and why the store's file did not load, when it did not.
#[derive(Debug)]
pub(crate) struct OpenedPreferences {
    pub(crate) preferences: Preferences,
    pub(crate) store: PreferencesStore,
    pub(crate) failure: Option<PreferencesLoadError>,
}

/// Where a [`PreferencesStore::save`] put the preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Saved {
    /// Into the store's file.
    File,
    /// Nowhere: a memory store keeps them in the session alone.
    Memory,
}

impl PreferencesStore {
    /// The user's preferences file, in the platform's configuration directory.
    pub(crate) fn user() -> Self {
        Self::File(
            platform::config_dir()
                .unwrap_or_default()
                .join(PREFERENCES_FILE),
        )
    }

    /// The preferences this store holds — a missing file reads as the defaults, as on a first
    /// run — and the store the session persists them to: this one, or [`Self::Memory`] when the
    /// file did not load, so the file stays as the user has it.
    pub(crate) fn open(self) -> OpenedPreferences {
        let loaded = match &self {
            Self::File(path) => Preferences::load_from(path),
            Self::Memory => Ok(Preferences::default()),
        };
        match loaded {
            Ok(preferences) => OpenedPreferences {
                preferences,
                store: self,
                failure: None,
            },
            Err(failure) => OpenedPreferences {
                preferences: Preferences::default(),
                store: Self::Memory,
                failure: Some(failure),
            },
        }
    }

    /// Persist `preferences` here, which a memory store does by keeping nothing.
    pub(crate) fn save(&self, preferences: &Preferences) -> Result<Saved, PreferencesSaveError> {
        match self {
            Self::File(path) => preferences.save_to(path).map(|()| Saved::File),
            Self::Memory => Ok(Saved::Memory),
        }
    }
}
