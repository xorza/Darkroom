//! Menu-command side effects: the file/theme/subgraph load-save flows
//! `App` runs *outside* the record pass (after the frame's record +
//! drain), so the blocking file dialog holds no frame borrows. Kept
//! apart from the per-frame pipeline in `app.rs` — these only touch
//! `Document`/`Theme`/`AppConfig` + persistence, never the gesture or
//! scene state.

use std::path::{Path, PathBuf};

use palantir::Ui;

use crate::app::App;
use crate::config::AppConfig;
use crate::document::Document;
use crate::gui::menu_bar::MenuCommand;
use crate::persistence;

impl App {
    pub(crate) fn handle_menu_command(&mut self, ui: &mut Ui, command: MenuCommand) {
        match command {
            MenuCommand::NewDocument => self.new_document(),
            MenuCommand::LoadDocument => {
                if let Some(path) = persistence::pick_open_path(self.current_path.as_deref()) {
                    self.load_document(&path);
                }
            }
            MenuCommand::SaveDocument => self.save_current(),
            MenuCommand::SaveDocumentAs => self.save_document_as(),
            MenuCommand::LoadTheme => {
                if let Some(path) = persistence::pick_theme_open() {
                    self.load_theme(ui, &path);
                }
            }
            MenuCommand::ExportTheme => {
                if let Some(path) = persistence::pick_theme_save() {
                    persistence::export_theme(&self.theme, &path);
                }
            }
            MenuCommand::ExportSubgraph => self.export_active_subgraph(),
            MenuCommand::ImportSubgraph => self.import_subgraph(),
        }
    }

    /// Export a subgraph def to a file (its interior `Graph` carries any
    /// nested subgraph defs along). A selected subgraph-instance node
    /// wins; otherwise, when the active tab is itself a subgraph, that
    /// open subgraph is exported. No-op when neither resolves.
    fn export_active_subgraph(&mut self) {
        let Some(def) = self.document.subgraph_to_export(&self.func_lib) else {
            eprintln!("subgraph export: no subgraph selected or open");
            return;
        };
        if let Some(path) = persistence::pick_save_path(self.current_path.as_deref()) {
            persistence::export_subgraph(def, &path);
        }
    }

    /// Import a subgraph def from a file as a local def in the current
    /// document. The import is a copy with a fresh id; nothing is
    /// instantiated and the undo stack is untouched (existing history
    /// references no imported def, so it stays valid).
    fn import_subgraph(&mut self) {
        let Some(path) = persistence::pick_open_path(self.current_path.as_deref()) else {
            return;
        };
        if let Some(def) = persistence::import_subgraph(&path) {
            self.document.import_subgraph(def);
        }
    }

    /// Replace the document with an empty one and reset all derived
    /// state. Clears the undo stack — restoring the previous doc via
    /// Cmd-Z would re-introduce all of its nodes one-step-at-a-time
    /// from intent history that no longer matches the live tree.
    fn new_document(&mut self) {
        self.document = Document::default();
        self.action_stack.clear();
        self.intents.clear();
        // Force a scene rebuild next frame: the active target may still
        // be `Main`, but it now points at a different graph.
        self.scene_target = None;
        self.set_document_path(None);
    }

    pub(crate) fn load_document(&mut self, path: &Path) {
        let Some(doc) = persistence::load_document(path) else {
            return;
        };
        self.document = doc;
        self.action_stack.clear();
        self.intents.clear();
        self.scene_target = None;
        self.set_document_path(Some(path.to_path_buf()));
    }

    /// Cmd+S: overwrite the current file if there is one, else fall
    /// back to Save As (first save of a fresh document).
    fn save_current(&mut self) {
        match self.current_path.clone() {
            Some(path) => self.save_document(&path),
            None => self.save_document_as(),
        }
    }

    /// Cmd+Shift+S / "Save As…": always prompt for a destination.
    fn save_document_as(&mut self) {
        if let Some(path) = persistence::pick_save_path(self.current_path.as_deref()) {
            self.save_document(&path);
        }
    }

    fn save_document(&mut self, path: &Path) {
        if persistence::save_document(&self.document, path) {
            self.set_document_path(Some(path.to_path_buf()));
        }
    }

    /// Record `path` as both the dialog-anchor `current_path` and the
    /// persisted `config.document_path`, then write the config so the
    /// next launch reopens this document.
    fn set_document_path(&mut self, path: Option<PathBuf>) {
        self.current_path = path.clone();
        self.config.document_path = path;
        self.config.save();
    }

    /// Load a theme picked from the dialog: copy it into the working
    /// dir under its own name (so the config can reference it by name
    /// across sessions), apply it, and persist the name. The picked
    /// file may live anywhere; the working-dir copy is the canonical
    /// one the config resolves on the next launch.
    fn load_theme(&mut self, ui: &mut Ui, picked: &Path) {
        let Some(stem) = picked.file_stem().and_then(|s| s.to_str()) else {
            eprintln!("theme load failed: path has no file name");
            return;
        };
        let dest = AppConfig::theme_path(stem);
        // Only copy when the picked file isn't already the working-dir
        // copy (re-loading the active theme shouldn't self-overwrite).
        if picked != dest
            && let Err(err) = std::fs::copy(picked, &dest)
        {
            eprintln!("theme load failed: copy to {}: {err}", dest.display());
            return;
        }
        if self.load_theme_file(&dest) {
            self.config.theme_name = Some(stem.to_owned());
            self.config.save();
            ui.theme = self.theme.palantir_theme.clone();
        }
    }

    /// Apply a theme `.toml` from `path` into `self.theme`. Returns
    /// whether it succeeded; on failure leaves the current theme
    /// untouched. Shared by startup restore and menu load.
    pub(crate) fn load_theme_file(&mut self, path: &Path) -> bool {
        match persistence::load_theme(path) {
            Some(theme) => {
                self.theme = theme;
                true
            }
            None => false,
        }
    }
}
