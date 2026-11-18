//! [`PreferencesStore`]: where a session's preferences persist.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use common::fs;

use crate::core::io::preferences::error::{
    PreferencesLoadError, PreferencesParseError, PreferencesReadError, PreferencesSaveError,
    PreferencesSetAsideError,
};
use crate::core::io::preferences::{PREFERENCES_FILE, Preferences};
use crate::platform;

/// The most backups of unparsable preferences files one folder holds: past them, a file that
/// does not parse stays where it is, and the session's changes stay in memory.
pub(super) const BACKUP_COUNT_MAX: u32 = 100;

/// Where a session reads its preferences from and writes its changes back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PreferencesStore {
    /// A preferences file: read at launch, rewritten on each change.
    File(PathBuf),
    /// Nowhere: the session starts on the defaults and its changes stay in memory. A session whose
    /// file could not be read, or did not parse and could not be set aside, runs on this, so the
    /// file stays as it is; so does a driven session, offscreen or in a window, and a harness
    /// session, which must change nothing on the machine.
    Memory,
}

/// What a session opens its preferences as: the preferences, the store their changes persist
/// to, and why the store's file did not load, when it did not.
#[derive(Debug)]
pub(crate) struct OpenedPreferences {
    pub(crate) preferences: Preferences,
    pub(crate) store: PreferencesStore,
    pub(crate) failure: Option<OpenFailure>,
}

/// Why a session opened on the defaults instead of its file's preferences.
#[derive(Debug)]
pub(crate) enum OpenFailure {
    /// The file did not parse and now lies at `backup`, so the session saves to the file again
    /// and nothing the file held is lost, as Firefox keeps a `prefs.js` that does not parse as
    /// `Invalidprefs.js` and starts afresh. Refused: keeping the session in memory, which drops
    /// every change made until the file is mended by hand.
    SetAside {
        failure: PreferencesParseError,
        backup: PathBuf,
    },
    /// The file did not parse and could not be set aside, so it stays as it is and the session
    /// runs on [`PreferencesStore::Memory`].
    Kept {
        failure: PreferencesParseError,
        set_aside: PreferencesSetAsideError,
    },
    /// The file could not be read, so it stays as it is and the session runs on
    /// [`PreferencesStore::Memory`]: what cannot be read cannot be set aside whole.
    Unreadable(PreferencesReadError),
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
    /// run — and the store the session persists them to. A file that does not parse is set aside
    /// ([`set_aside`]) and the session starts on the defaults in this store; one that cannot be
    /// read, or set aside, stays as it is, and the session runs on [`Self::Memory`].
    pub(crate) fn open(self) -> OpenedPreferences {
        let Self::File(path) = &self else {
            return OpenedPreferences {
                preferences: Preferences::default(),
                store: self,
                failure: None,
            };
        };
        let failure = match Preferences::load_from(path) {
            Ok(preferences) => {
                return OpenedPreferences {
                    preferences,
                    store: self,
                    failure: None,
                };
            }
            Err(PreferencesLoadError::Read(failure)) => OpenFailure::Unreadable(failure),
            Err(PreferencesLoadError::Parse(failure)) => match set_aside(path) {
                Ok(backup) => OpenFailure::SetAside { failure, backup },
                Err(set_aside) => OpenFailure::Kept { failure, set_aside },
            },
        };
        let store = match failure {
            OpenFailure::SetAside { .. } => self,
            OpenFailure::Kept { .. } | OpenFailure::Unreadable(_) => Self::Memory,
        };
        OpenedPreferences {
            preferences: Preferences::default(),
            store,
            failure: Some(failure),
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

/// Move the file at `path` to the first free backup name beside it, `<stem>.invalid.<ext>`, then
/// `<stem>.invalid-2.<ext>` up to [`BACKUP_COUNT_MAX`], and answer where it went. Each name is
/// first made empty and exclusively, so no two sessions take one and no backup is overwritten;
/// the move then replaces the empty file. One fixed name is refused: Firefox's single
/// `Invalidprefs.js` stopped every later backup once it existed
/// ([bug 495735](https://bugzilla.mozilla.org/show_bug.cgi?id=495735)).
fn set_aside(path: &Path) -> Result<PathBuf, PreferencesSetAsideError> {
    for n in 1..=BACKUP_COUNT_MAX {
        let backup = backup_name(path, n);
        match fs::create_new(&backup) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(PreferencesSetAsideError::Move {
                    path: backup,
                    source,
                });
            }
        }
        return match fs::rename(path, &backup) {
            Ok(()) => Ok(backup),
            Err(source) => {
                // Best effort: an empty reservation left behind only takes one name.
                fs::remove_file(&backup).ok();
                Err(PreferencesSetAsideError::Move {
                    path: backup,
                    source,
                })
            }
        };
    }
    Err(PreferencesSetAsideError::Exhausted {
        path: path.to_path_buf(),
    })
}

/// The `n`th backup name of `path`, from 1: `<stem>.invalid.<ext>`, then `<stem>.invalid-<n>.<ext>`.
pub(super) fn backup_name(path: &Path, n: u32) -> PathBuf {
    assert!(n >= 1, "backup names count from 1");
    let mut name = OsString::from(path.file_stem().unwrap_or_default());
    name.push(".invalid");
    if n > 1 {
        name.push(format!("-{n}"));
    }
    if let Some(ext) = path.extension() {
        name.push(".");
        name.push(ext);
    }
    path.with_file_name(name)
}
