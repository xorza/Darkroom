//! Preferences edits through the `Changed` synchronization sink and model picker.
//! `set_confirm_exit` is the one preference `App` also writes
//! from outside the tab (the exit dialog's "Don't ask again").

use std::path::PathBuf;

use aperture::Ui;
use scenarium::{FsPathConfig, FsPathMode};

use crate::gui::app::App;
use crate::gui::dialogs;
use crate::gui::theme::Theme;

/// Preferences UI actions. Handled by [`App::handle_prefs`] after authoring.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PrefsCommand {
    /// A field of [`crate::core::io::preferences::Preferences`] was edited
    /// in place — by the Preferences tab (any checkbox / radio / path field)
    /// or the image viewer's toolbar (backdrop / sampling). `App` synchronizes
    /// derived state and persists it — one command for every field, so adding
    /// a preference needs no new command.
    Changed,
    PickMlModel(MlModelKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MlModelKind {
    Denoise,
    StarRemoval,
}

impl App {
    pub(crate) fn handle_prefs(&mut self, ui: &mut Ui, command: PrefsCommand) {
        match command {
            PrefsCommand::Changed => self.apply_preferences(ui),
            PrefsCommand::PickMlModel(kind) => self.pick_ml_model(kind),
        }
    }

    /// Re-derive everything that depends on [`Preferences`] and persist it.
    ///
    /// [`Preferences`]: crate::core::io::preferences::Preferences
    fn apply_preferences(&mut self, ui: &mut Ui) {
        self.theme = Theme::from_preset(self.preferences.theme.resolve());
        ui.theme = self.theme.aperture_theme.clone();
        let paths = (&self.preferences.ml_models).into();
        self.engine.configure_ml_model_defaults(&paths);
        self.save_preferences();
    }

    /// Persist the preferences, surfacing a failed write in the status
    /// bar — the one save path every caller routes through, so a broken
    /// preferences file can't fail silently.
    pub(crate) fn save_preferences(&mut self) {
        if let Err(err) = self.preferences.save() {
            self.engine.status.error(err);
        }
    }

    fn pick_ml_model(&mut self, kind: MlModelKind) {
        let filter =
            FsPathConfig::with_extensions(FsPathMode::ExistingFile, vec!["onnx".to_string()]);
        if let Some(path) = dialogs::pick_path(&filter) {
            self.set_ml_model_path(kind, path);
        }
    }

    fn set_ml_model_path(&mut self, kind: MlModelKind, path: PathBuf) {
        match kind {
            MlModelKind::Denoise => self.preferences.ml_models.denoise = path,
            MlModelKind::StarRemoval => self.preferences.ml_models.star_removal = path,
        }
        let paths = (&self.preferences.ml_models).into();
        self.engine.configure_ml_model_defaults(&paths);
        self.save_preferences();
    }

    /// Persist whether quitting with unsaved changes prompts to save.
    /// Shared by the Preferences checkbox (via `Changed`) and the exit
    /// dialog's "Don't ask again", which calls this directly.
    pub(crate) fn set_confirm_exit(&mut self, on: bool) {
        self.preferences.confirm_unsaved_on_exit = on;
        self.save_preferences();
    }
}
