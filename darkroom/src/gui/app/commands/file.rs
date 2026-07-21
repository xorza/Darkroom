//! Document file lifecycle: new / load / save / save-as, plus the shared
//! document-path sink that repoints the dialog anchor, the worker's
//! disk cache, and the persisted last-document.

use std::path::Path;

use crate::core::open_document::OpenDocument;
use crate::gui::app::App;
use crate::gui::app::editor::Editor;
use crate::gui::dialogs;

/// Document file lifecycle. Handled by [`App::handle_file`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum FileCommand {
    /// Replace the document with an empty one.
    New,
    /// Prompt for a file and load it.
    Load,
    /// Save to the current file, or prompt (Save As) if there isn't one.
    Save,
    /// Always prompt for a destination.
    SaveAs,
}

impl App {
    pub(crate) fn handle_file(&mut self, command: FileCommand) {
        match command {
            FileCommand::New => self.new_document(),
            FileCommand::Load => {
                if let Some(path) =
                    dialogs::pick_project_open_path(self.workspace.open.path.as_deref())
                {
                    self.load_document(&path);
                }
            }
            FileCommand::Save => self.save_current(),
            FileCommand::SaveAs => self.save_document_as(),
        }
    }

    /// Replace the document with an empty one. A fresh [`Editor`] resets
    /// all derived/transient state in one move: empty undo history
    /// (restoring the old doc via Cmd-Z would replay nodes from intent
    /// history that no longer matches the live tree), forced reconcile +
    /// scene rebuild, dropped gesture state, and cleared run results.
    fn new_document(&mut self) {
        self.editor = Editor::new();
        self.workspace.replace_document(OpenDocument::default());
        self.remember_document_path();
    }

    /// Load `path` into a fresh editor. Returns whether it loaded — `false`
    /// when the file is missing or corrupt, leaving the open document intact.
    /// The failure surfaces in the status bar with its reason.
    pub(crate) fn load_document(&mut self, path: &Path) -> bool {
        let open = match OpenDocument::load(path.to_path_buf()) {
            Ok(open) => open,
            Err(err) => {
                self.workspace
                    .runtime
                    .status
                    .error(format!("load failed: {err:#}"));
                return false;
            }
        };
        // Fresh editor around the loaded doc — see `new_document` for why
        // a wholesale reset (rather than poking individual fields) is right.
        self.editor = Editor::new();
        self.workspace.replace_document(open);
        self.remember_document_path();
        self.workspace.runtime.status.error = None;
        true
    }

    /// Cmd+S: overwrite the current file if there is one, else fall
    /// back to Save As (first save of a fresh document).
    pub(crate) fn save_current(&mut self) {
        match self.workspace.open.path.clone() {
            Some(path) => self.save_document(&path),
            None => self.save_document_as(),
        }
    }

    /// Cmd+Shift+S / "Save As…": always prompt for a destination.
    fn save_document_as(&mut self) {
        if let Some(path) = dialogs::pick_project_save_path(self.workspace.open.path.as_deref()) {
            self.save_document(&path);
        }
    }

    fn save_document(&mut self, path: &Path) {
        match self.workspace.save_to(path) {
            Ok(()) => {
                self.editor.dirty = false;
                self.remember_document_path();
                self.workspace.runtime.status.error = None;
            }
            Err(err) => self
                .workspace
                .runtime
                .status
                .error(format!("save failed: {err:#}")),
        }
    }

    /// Mirror the workspace's active path into persisted preferences after a
    /// successful document lifecycle transition.
    fn remember_document_path(&mut self) {
        self.preferences.document_path = self.workspace.open.path.clone();
        self.save_preferences();
    }
}
