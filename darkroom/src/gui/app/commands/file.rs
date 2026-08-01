//! Document file lifecycle: new / load / save / save-as, plus the shared
//! document-path sink that repoints the dialog anchor, the worker's
//! disk cache, and the persisted last-document.

use std::path::Path;

use crate::core::document::open_document::OpenDocument;
use crate::gui::app::editor::Editor;
use crate::gui::app::{App, PendingTransition};
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
    pub(super) fn handle_file(&mut self, command: FileCommand) {
        match command {
            // Both replace the open document, so they clear the
            // unsaved-changes guard before doing anything.
            FileCommand::New => self.guard_discard(PendingTransition::New),
            FileCommand::Load => self.guard_discard(PendingTransition::Load),
            FileCommand::Save => self.save_current(),
            FileCommand::SaveAs => self.save_document_as(),
        }
    }

    /// Prompt for a project file and load it. The [`PendingTransition::Load`]
    /// body: the picker runs here, *after* the guard cleared, so cancelling
    /// the unsaved-changes prompt never costs the user a file choice.
    pub(crate) fn load_picked_document(&mut self) {
        if let Some(path) = dialogs::pick_project_open_path(self.open.path.as_deref()) {
            self.load_document(&path);
        }
    }

    /// Replace the document with an empty one.
    pub(crate) fn new_document(&mut self) {
        self.adopt_document(OpenDocument::default());
    }

    /// Swap in `open` and reset every piece of state derived from the
    /// document it replaces.
    ///
    /// A fresh [`Editor`] covers what it owns in one move: empty undo history
    /// (restoring the old doc via Cmd-Z would replay intents that no longer
    /// match the live tree), dropped gesture state, forced scene rebuild.
    ///
    /// The run projections are `App`'s, so they are cleared here rather than
    /// falling out of that — and they have to be: node ids are *persisted*, so
    /// reopening a document would otherwise reattach the previous session's
    /// statuses, timings, logs and preview images to nodes that have not run.
    /// [`RunState::clear`](crate::gui::state::run_state::RunState::clear) drops
    /// exactly the document-derived half and leaves
    /// the worker-stream half (`compiled`, `activity`, `cache_ram`) standing —
    /// an in-flight run still reports against the program the worker
    /// acknowledged, and its cache really does still hold that RAM.
    ///
    /// The worker's disk cache repoints too, so disk-backed nodes read the new
    /// document's store rather than the old one's.
    fn adopt_document(&mut self, open: OpenDocument) {
        self.editor = Editor::new();
        self.run_state.clear();
        self.open = open;
        self.runtime.set_document_cache(self.open.path.as_deref());
        self.remember_document_path();
    }

    /// Load `path` into a fresh editor. A missing or corrupt file leaves the
    /// open document intact and surfaces its reason in the status bar.
    fn load_document(&mut self, path: &Path) {
        let open = match OpenDocument::load(path.to_path_buf()) {
            Ok(open) => open,
            Err(err) => {
                self.runtime.status.error(format!("load failed: {err:#}"));
                return;
            }
        };
        self.adopt_document(open);
        self.runtime.status.error = None;
    }

    /// Cmd+S: overwrite the current file if there is one, else fall
    /// back to Save As (first save of a fresh document).
    pub(crate) fn save_current(&mut self) {
        match self.open.path.clone() {
            Some(path) => self.save_document(&path),
            None => self.save_document_as(),
        }
    }

    /// Cmd+Shift+S / "Save As…": always prompt for a destination.
    fn save_document_as(&mut self) {
        if let Some(path) = dialogs::pick_project_save_path(self.open.path.as_deref()) {
            self.save_document(&path);
        }
    }

    /// Write the document to `path` and adopt it as the save target. Save-As
    /// moves the document, so the worker's disk cache repoints to the new
    /// location's store — the old one stays where it is.
    fn save_document(&mut self, path: &Path) {
        match self.open.save_to(path) {
            Ok(()) => {
                self.runtime.set_document_cache(self.open.path.as_deref());
                self.remember_document_path();
                self.runtime.status.error = None;
            }
            Err(err) => self.runtime.status.error(format!("save failed: {err:#}")),
        }
    }

    /// Mirror the open document's active path into persisted preferences
    /// after a successful document lifecycle transition.
    fn remember_document_path(&mut self) {
        self.preferences.document_path = self.open.path.clone();
        self.save_preferences();
    }
}
