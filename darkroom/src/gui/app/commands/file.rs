//! Document file lifecycle: new / load / save / save-as, plus the shared
//! `set_document_path` sink that repoints the dialog anchor, the worker's
//! disk cache, and the persisted last-document.

use std::path::{Path, PathBuf};

use crate::core::document::Document;
use crate::core::io::persistence;
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
                if let Some(path) = dialogs::pick_open_path(self.current_path.as_deref()) {
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
        self.editor = Editor::new(Document::default());
        self.set_document_path(None);
    }

    /// Load `path` into a fresh editor. Returns whether it loaded — `false`
    /// when the file is missing/corrupt (startup uses this to drop a stale
    /// `document_path`; the menu-load path ignores it, leaving the open doc).
    /// The failure surfaces in the status bar; the detail is in the log.
    pub(crate) fn load_document(&mut self, path: &Path) -> bool {
        let Some(doc) = persistence::load_document(path) else {
            self.report_error(format!("load failed: {}", path.display()));
            return false;
        };
        // Fresh editor around the loaded doc — see `new_document` for why
        // a wholesale reset (rather than poking individual fields) is right.
        self.editor = Editor::new(doc);
        self.set_document_path(Some(path.to_path_buf()));
        self.status_error = None;
        true
    }

    /// Cmd+S: overwrite the current file if there is one, else fall
    /// back to Save As (first save of a fresh document).
    pub(crate) fn save_current(&mut self) {
        match self.current_path.clone() {
            Some(path) => self.save_document(&path),
            None => self.save_document_as(),
        }
    }

    /// Cmd+Shift+S / "Save As…": always prompt for a destination.
    fn save_document_as(&mut self) {
        if let Some(path) = dialogs::pick_save_path(self.current_path.as_deref()) {
            self.save_document(&path);
        }
    }

    fn save_document(&mut self, path: &Path) {
        if persistence::save_document(&self.editor.document, path) {
            self.editor.dirty = false;
            self.set_document_path(Some(path.to_path_buf()));
            self.status_error = None;
        } else {
            self.report_error(format!("save failed: {}", path.display()));
        }
    }

    /// Record `path` as both the dialog-anchor `current_path` and the
    /// persisted `preferences.document_path`, then write the preferences so the
    /// next launch reopens this document. Also repoints the worker's disk
    /// cache at the document's project-local store (or memory-only when the
    /// path is cleared / never saved).
    fn set_document_path(&mut self, path: Option<PathBuf>) {
        self.current_path = path.clone();
        self.engine.set_document_cache(self.current_path.as_deref());
        self.preferences.document_path = path;
        self.preferences.save();
    }
}
