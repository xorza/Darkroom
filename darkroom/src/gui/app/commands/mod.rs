//! [`AppCommand`](crate::gui::app::AppCommand) handling: the file / subgraph /
//! run / preferences / edit / shell side effects `App` runs *outside* the
//! record pass (after the frame's record + drain), so a blocking file dialog
//! or worker call holds no frame borrows.
//!
//! [`App::handle_command`] is a thin dispatcher — each top-level command group
//! resolves to one submodule's `impl App` block (`file`, `subgraph`, `run`,
//! `prefs`, `edit`, `shell`). The commands are cross-subsystem coordination
//! (they bridge `Document` / `Engine` / `Preferences` / dialogs), which is why
//! they live on `App` rather than any one owner; the split is by concern.

use palantir::Ui;

use crate::gui::app::App;
use crate::gui::app::AppCommand;

mod edit;
mod file;
mod prefs;
mod run;
mod shell;
mod subgraph;

impl App {
    /// Dispatch a deferred command to its group handler. Runs after the
    /// frame's record + drain (see the module docs) with `ui` available for
    /// the handful of handlers that re-sync `Ui` state (e.g. theme).
    pub(crate) fn handle_command(&mut self, ui: &mut Ui, command: AppCommand) {
        match command {
            AppCommand::File(c) => self.handle_file(c),
            AppCommand::Subgraph(c) => self.handle_subgraph(c),
            AppCommand::Run(c) => self.handle_run(c),
            AppCommand::Prefs(c) => self.handle_prefs(ui, c),
            AppCommand::Edit(c) => self.handle_edit(c),
            AppCommand::Shell(c) => self.handle_shell(c),
        }
    }
}
