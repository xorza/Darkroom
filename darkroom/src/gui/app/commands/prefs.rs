//! Preferences edits through the `Changed` synchronization sink and model picker.
//! `set_confirm_unsaved` is the one preference `App` also writes
//! from outside the tab (the exit dialog's "Don't ask again").

use crate::gui::app::App;

/// Preferences UI actions. Applied by [`PrefsCommand::apply`] after authoring.
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

impl PrefsCommand {
    pub(super) fn apply(self, app: &mut App) {
        match self {
            PrefsCommand::Changed => app.apply_preferences(),
            PrefsCommand::PickMlModel(kind) => app.pick_ml_model(kind),
        }
    }
}
