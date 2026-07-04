//! Preferences edits: the single `Changed` re-sync sink (theme palette +
//! ML paths + persist) plus the ML-model file dialog. `set_confirm_exit` is
//! the one preference `App` also writes from outside the tab (the exit
//! dialog's "Don't ask again").

use std::path::PathBuf;

use palantir::Ui;
use scenarium::data::{FsPathConfig, FsPathMode};

use crate::gui::app::App;
use crate::gui::dialogs;
use crate::gui::theme::Theme;

/// Preferences edits. Handled by [`App::handle_prefs`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum PrefsCommand {
    /// The Preferences tab edited a field of [`crate::core::io::preferences::Preferences`]
    /// in place (any checkbox / radio / path field). `App` re-syncs derived
    /// state (theme palette, ML paths) and persists — one command for every
    /// field, so adding a preference needs no new command.
    Changed,
    /// Open an ONNX file dialog for one of the ML model paths (the "Browse…"
    /// buttons) — the blocking dialog runs outside the record, unlike the
    /// in-place field edits that report [`Self::Changed`].
    PickMlModel(MlModelKind),
}

/// Which ML model path a [`PrefsCommand::PickMlModel`] targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MlModelKind {
    /// The `ml_denoise` node's model (DeepSNR).
    Denoise,
    /// The `remove_stars` node's model (StarNet).
    StarRemoval,
}

impl App {
    pub(crate) fn handle_prefs(&mut self, ui: &mut Ui, command: PrefsCommand) {
        match command {
            PrefsCommand::Changed => self.apply_preferences(ui),
            PrefsCommand::PickMlModel(kind) => self.pick_ml_model(kind),
        }
    }

    /// Re-derive everything that depends on [`Preferences`] and persist —
    /// the single sink for the Preferences tab's in-place edits, so every
    /// field flows through one path (no per-field command). Re-resolves the
    /// theme palette (`System` queries the OS) onto `self.theme` + the `Ui`,
    /// republishes the ML paths to lens, and writes the file. Idempotent, so
    /// running it for a field whose derived bit didn't move is harmless.
    ///
    /// [`Preferences`]: crate::core::io::preferences::Preferences
    fn apply_preferences(&mut self, ui: &mut Ui) {
        self.theme = Theme::from_preset(self.preferences.theme.resolve());
        ui.theme = self.theme.palantir_theme.clone();
        self.preferences.apply_ml_model_paths();
        self.preferences.save();
    }

    /// Open an ONNX file dialog for one of the ML model paths and, on a
    /// pick, record it (persist + republish to lens). Runs outside the
    /// record (blocking dialog), like the other file ops.
    fn pick_ml_model(&mut self, kind: MlModelKind) {
        let filter =
            FsPathConfig::with_extensions(FsPathMode::ExistingFile, vec!["onnx".to_string()]);
        if let Some(path) = dialogs::pick_path(&filter) {
            self.set_ml_model_path(kind, path);
        }
    }

    /// Record `path` for `kind`, persist the preferences, and republish the
    /// paths to lens so the next node run uses it.
    fn set_ml_model_path(&mut self, kind: MlModelKind, path: PathBuf) {
        match kind {
            MlModelKind::Denoise => self.preferences.ml_models.denoise = path,
            MlModelKind::StarRemoval => self.preferences.ml_models.star_removal = path,
        }
        self.preferences.save();
        self.preferences.apply_ml_model_paths();
    }

    /// Persist whether quitting with unsaved changes prompts to save.
    /// Shared by the Preferences checkbox (via `Changed`) and the exit
    /// dialog's "Don't ask again", which calls this directly.
    pub(crate) fn set_confirm_exit(&mut self, on: bool) {
        self.preferences.confirm_unsaved_on_exit = on;
        self.preferences.save();
    }
}
