//! App shell: navigation + lifecycle. Opening the Preferences tab and the
//! quit request (which routes through the unsaved-changes prompt).

use crate::gui::app::App;
use crate::gui::app::ShellCommand;

impl App {
    pub(crate) fn handle_shell(&mut self, command: ShellCommand) {
        match command {
            ShellCommand::OpenPreferences => {
                let library = self.engine.library.load();
                self.editor.open_preferences(&library);
            }
            ShellCommand::Quit => self.request_quit(),
        }
    }

    /// A quit was requested (File ▸ Quit, ⌘Q). Prompt to save when the
    /// document has unsaved changes *and* the confirm-on-exit preference is
    /// on; otherwise exit now. The confirm dialog is rendered and resolved
    /// by [`App::handle_exit`].
    fn request_quit(&mut self) {
        if self.editor.dirty && self.preferences.confirm_unsaved_on_exit {
            self.confirm_quit = true;
        } else {
            self.quit();
        }
    }
}
