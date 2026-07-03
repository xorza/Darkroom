//! The "save changes before quitting?" confirmation dialog. Rendered by
//! [`App::handle_exit`](super::App) when a quit is requested with unsaved
//! changes; the returned [`ExitChoice`] drives whether the app saves,
//! discards, or stays.

use palantir::{Button, Configure, Modal, Panel, Text, Ui};

/// The user's answer to the unsaved-changes prompt for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitChoice {
    /// No button pressed yet — keep the dialog up.
    Stay,
    /// Keep editing (Cancel button, Esc, or backdrop click).
    Cancel,
    /// Quit without saving.
    Discard,
    /// Save first, then quit.
    Save,
}

/// Render the modal over the current frame. `file_name` names the document
/// in the prompt (`None` for a never-saved one). Returns the choice the
/// user made this frame.
pub(crate) fn show(ui: &mut Ui, file_name: Option<&str>) -> ExitChoice {
    let title = match file_name {
        Some(name) => ui.fmt(format_args!("Save changes to “{name}” before quitting?")),
        None => ui.fmt(format_args!("Save changes before quitting?")),
    };

    let mut choice = ExitChoice::Stay;
    let resp = Modal::new()
        .id_salt(("exit_dialog", "modal"))
        .show(ui, |ui| {
            Panel::vstack()
                .id_salt(("exit_dialog", "body"))
                .gap(16.0)
                .padding(8.0)
                .show(ui, |ui| {
                    Text::new(title).id_salt(("exit_dialog", "title")).show(ui);
                    Panel::hstack()
                        .id_salt(("exit_dialog", "row"))
                        .gap(8.0)
                        .show(ui, |ui| {
                            if Button::new()
                                .id_salt(("exit_dialog", "save"))
                                .label("Save")
                                .show(ui)
                                .clicked()
                            {
                                choice = ExitChoice::Save;
                            }
                            if Button::new()
                                .id_salt(("exit_dialog", "discard"))
                                .label("Don't Save")
                                .show(ui)
                                .clicked()
                            {
                                choice = ExitChoice::Discard;
                            }
                            if Button::new()
                                .id_salt(("exit_dialog", "cancel"))
                                .label("Cancel")
                                .show(ui)
                                .clicked()
                            {
                                choice = ExitChoice::Cancel;
                            }
                        });
                });
        });
    // Esc / backdrop click dismisses the modal — treat as Cancel.
    if resp.dismissed {
        choice = ExitChoice::Cancel;
    }
    choice
}
