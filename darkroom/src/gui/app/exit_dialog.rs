//! The "save changes before quitting?" confirmation dialog. Rendered by
//! [`App::record_exit`](crate::gui::app::App::record_exit) when a quit is
//! requested with unsaved changes; the returned [`ExitOutcome`] is applied
//! immediately after the dialog finishes authoring.

use aperture::{Button, Checkbox, Configure, Modal, Panel, Text, Ui, WidgetId};

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

/// What the exit dialog reported this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExitOutcome {
    pub choice: ExitChoice,
    /// "Don't ask again" checkbox state. Honored only when `choice` is a
    /// proceed action (`Save`/`Discard`) — a `Cancel` leaves the
    /// preference untouched.
    pub dont_ask_again: bool,
}

/// Render the modal over the current frame. `file_name` names the document
/// in the prompt (`None` for a never-saved one). Returns the choice the
/// user made this frame plus the "Don't ask again" state.
pub(crate) fn show(ui: &mut Ui, file_name: Option<&str>) -> ExitOutcome {
    let title = match file_name {
        Some(name) => ui.fmt(format_args!("Save changes to “{name}” before quitting?")),
        None => ui.fmt(format_args!("Save changes before quitting?")),
    };

    // Checkbox state lives across the frames the dialog is up; the id isn't
    // recorded once the dialog closes, so the row is swept and the next
    // open starts unchecked.
    let dont_ask_id = WidgetId::from_hash("exit_dialog::dont_ask_again");
    let mut dont_ask_again = *ui.state_mut::<bool>(dont_ask_id);

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
                    Checkbox::new(&mut dont_ask_again)
                        .id_salt(("exit_dialog", "dont_ask"))
                        .label("Don't ask again")
                        .show(ui);
                    Panel::hstack()
                        .id_salt(("exit_dialog", "row"))
                        .gap(8.0)
                        .show(ui, |ui| {
                            if Button::new()
                                .id_salt(("exit_dialog", "save"))
                                .label("Save")
                                .show(ui)
                                .left
                                .clicked()
                            {
                                choice = ExitChoice::Save;
                            }
                            if Button::new()
                                .id_salt(("exit_dialog", "discard"))
                                .label("Don't Save")
                                .show(ui)
                                .left
                                .clicked()
                            {
                                choice = ExitChoice::Discard;
                            }
                            if Button::new()
                                .id_salt(("exit_dialog", "cancel"))
                                .label("Cancel")
                                .show(ui)
                                .left
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

    *ui.state_mut::<bool>(dont_ask_id) = dont_ask_again;
    ExitOutcome {
        choice,
        dont_ask_again,
    }
}
