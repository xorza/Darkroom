//! [`AppCommand`] handling: the file / subgraph /
//! run / preferences / edit / shell side effects `App` runs *outside* the
//! record pass (after the frame's record + drain), so a blocking file dialog
//! or worker call holds no frame borrows.
//!
//! [`App::handle_command`] is a thin dispatcher — each top-level command group
//! resolves to one submodule's `impl App` block (`file`, `subgraph`, `run`,
//! `prefs`, `edit`, `shell`). The commands are cross-subsystem coordination
//! (they bridge `Document` / `Engine` / `Preferences` / dialogs), which is why
//! they live on `App` rather than any one owner; the split is by concern.

use aperture::Ui;

use crate::gui::app::App;

pub(crate) mod edit;
pub(crate) mod file;
pub(crate) mod prefs;
pub(crate) mod run;
pub(crate) mod shell;
pub(crate) mod subgraph;

use edit::EditCommand;
use file::FileCommand;
use prefs::PrefsCommand;
use run::RunCommand;
use shell::ShellCommand;
use subgraph::SubgraphCommand;

/// A deferred, side-effecting command a UI surface (the menu bar, the graph
/// toolbar, the Preferences tab, a node's S-badge, an inline path-picker)
/// hands to [`App`] to perform *outside* the record pass — after the frame's
/// record + drain, so a blocking file dialog or worker call holds no frame
/// borrows. The producing UI never touches `Document` / `Theme` / `Engine`
/// directly; it returns one of these and [`App::handle_command`] dispatches
/// it to the matching group handler (one submodule of `gui::app::commands`
/// per variant here).
#[derive(Clone, Debug)]
pub(crate) enum AppCommand {
    /// Document file lifecycle — [`file`].
    File(FileCommand),
    /// Subgraph → library publishing — [`subgraph`].
    Subgraph(SubgraphCommand),
    /// Graph execution + worker event loop — [`run`].
    Run(RunCommand),
    /// Preferences edits — [`prefs`].
    Prefs(PrefsCommand),
    /// Node edits raised via a dialog — [`edit`].
    Edit(EditCommand),
    /// App shell: navigation + lifecycle — [`shell`].
    Shell(ShellCommand),
}

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
