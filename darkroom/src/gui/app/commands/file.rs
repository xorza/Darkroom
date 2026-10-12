//! Document file lifecycle: new / open / save / save-as, plus the shared
//! document-path sink that repoints the dialog anchor, the worker's
//! disk cache, and the persisted last-document.

use crate::gui::app::{App, PendingTransition};

/// Document file lifecycle. Applied by [`FileCommand::apply`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum FileCommand {
    /// Replace the document with an empty one.
    New,
    /// Prompt for a file and open it.
    Open,
    /// Save to the current file, or prompt (Save As) if there isn't one.
    Save,
    /// Always prompt for a destination.
    SaveAs,
}

impl FileCommand {
    pub(super) fn apply(self, app: &mut App) {
        match self {
            // Both replace the open document, so they clear the
            // unsaved-changes guard before doing anything.
            Self::New => app.guard_discard(PendingTransition::New),
            Self::Open => app.guard_discard(PendingTransition::OpenPicked),
            Self::Save => app.save_current(),
            Self::SaveAs => app.save_document_as(),
        }
    }
}
