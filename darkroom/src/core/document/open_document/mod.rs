//! The document currently open in a frontend, with its persistence path.

use std::path::{Path, PathBuf};

use crate::core::document::Document;
use crate::core::io::document::{self, DocumentLoadError, DocumentSaveError};
use crate::core::io::preferences::Preferences;
use crate::core::status::StatusLog;

#[derive(Debug, Default)]
pub(crate) struct OpenDocument {
    pub(crate) document: Document,
    pub(crate) path: Option<PathBuf>,
    /// Whether `document` differs from what is at `path` — the pair that
    /// gives the flag its meaning, which is why it lives here rather than on
    /// a frontend. Set by any content-changing edit (new edits, undo/redo
    /// replay, direct graph mutations), cleared by [`Self::save_to`]. Pure
    /// navigation (camera, selection, pane arrangement) leaves it alone; see
    /// [`UndoStep::dirties_document`](crate::core::edit::intent::types::UndoStep::dirties_document).
    ///
    /// It can read "dirty" after an undo returns the document to its saved
    /// state — the safe direction (prompt rather than silently discard).
    pub(crate) dirty: bool,
}

impl OpenDocument {
    pub(crate) fn load(path: PathBuf) -> Result<Self, DocumentLoadError> {
        let document = document::load(&path)?;
        Ok(Self {
            document,
            path: Some(path),
            dirty: false,
        })
    }

    /// The document a launching frontend restores: the one `preferences`
    /// remembers, or an empty one when there is none or reopening is
    /// switched off. A failed load is reported to `status` and forgets the
    /// remembered path, so the next launch starts clean instead of failing
    /// again — which is why this takes the preferences by `&mut` and
    /// persists them.
    pub(crate) fn load_preferred(preferences: &mut Preferences, status: &mut StatusLog) -> Self {
        Self::load_preferred_with(preferences, status, Preferences::save)
    }

    /// [`Self::load_preferred`] with the preferences write injected, so a
    /// test can drive the path where forgetting the bad document fails too.
    fn load_preferred_with(
        preferences: &mut Preferences,
        status: &mut StatusLog,
        save_preferences: impl FnOnce(&Preferences) -> Result<(), String>,
    ) -> Self {
        let Some(path) = preferences
            .document_path
            .clone()
            .filter(|_| preferences.load_last_document)
        else {
            return Self::default();
        };
        match Self::load(path) {
            Ok(open) => open,
            Err(error) => {
                status.error(format!("load failed: {error:#}"));
                preferences.document_path = None;
                if let Err(error) = save_preferences(preferences) {
                    status.error(error);
                }
                Self::default()
            }
        }
    }

    /// Write the document to `path` and adopt it. Clears
    /// [`dirty`](Self::dirty) — only on success, so a failed save leaves the
    /// unsaved work still flagged.
    pub(crate) fn save_to(&mut self, path: &Path) -> Result<(), DocumentSaveError> {
        document::save(&self.document, path)?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
