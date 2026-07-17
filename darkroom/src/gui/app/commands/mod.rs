//! [`AppCommand`] handling: file / subgraph / run / preferences / edit / shell
//! side effects. Commands are produced by action input, which Aperture exposes
//! only to the first record pass, so handlers can run directly after authoring.
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

/// A command a UI surface (the menu bar, the graph toolbar, the Preferences
/// tab, a node's S-badge, an inline path-picker) hands to [`App`]. The producing
/// UI never touches `Document` / `Theme` / `Engine` directly.
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
    /// Dispatch a command after the editor has finished authoring its pass.
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
