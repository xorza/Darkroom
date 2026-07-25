//! App shell: navigation + lifecycle. Opening the Preferences tab and the
//! quit request (which routes through the unsaved-changes prompt).

use crate::gui::app::{App, PendingAction};

/// App shell: navigation + lifecycle. Handled by [`App::handle_shell`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShellCommand {
    /// Open (or focus) the Preferences tab — the app-settings window.
    OpenPreferences,
    /// Quit the app. Routed through `App::guard_discard`, which prompts to
    /// save first if the document has unsaved changes.
    Quit,
}

impl App {
    pub(super) fn handle_shell(&mut self, command: ShellCommand) {
        match command {
            ShellCommand::OpenPreferences => {
                self.editor.open_preferences(&mut self.workspace.open);
            }
            ShellCommand::Quit => self.guard_discard(PendingAction::Quit),
        }
    }
}
