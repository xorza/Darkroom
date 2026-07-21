//! App shell: navigation + lifecycle. Opening the Preferences tab and the
//! quit request (which routes through the unsaved-changes prompt).

use crate::gui::app::App;

/// App shell: navigation + lifecycle. Handled by [`App::handle_shell`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum ShellCommand {
    /// Open (or focus) the Preferences tab — the app-settings window.
    OpenPreferences,
    /// Quit the app. Routed through `App::request_quit`, which prompts to
    /// save first if the document has unsaved changes.
    Quit,
}

impl App {
    pub(crate) fn handle_shell(&mut self, command: ShellCommand) {
        match command {
            ShellCommand::OpenPreferences => {
                let library = self.engine.library.current.clone();
                self.editor.open_preferences(&library);
            }
            ShellCommand::Quit => self.request_quit(),
        }
    }

    /// A quit was requested (File ▸ Quit, ⌘Q). Prompt to save when the
    /// document has unsaved changes *and* the confirm-on-exit preference is
    /// on; otherwise exit now. [`App::record_exit`] renders and resolves the
    /// confirm dialog.
    fn request_quit(&mut self) {
        if self.needs_exit_confirmation() {
            self.confirm_quit = true;
        } else {
            self.quit();
        }
    }
}
