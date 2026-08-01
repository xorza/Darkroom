//! [`AppCommand`] handling: file / run / preferences / edit side effects, plus
//! the quit request. Commands are produced by action input, which Palantir
//! exposes only to the first record pass, so handlers can run directly after
//! authoring.
//!
//! [`App::handle_command`] is a thin dispatcher — each top-level command group
//! resolves to one submodule's `impl App` block (`file`, `run`, `prefs`,
//! `edit`); `Quit` carries no payload and resolves here. The commands are
//! cross-subsystem coordination (they bridge the open document /
//! `RuntimeHost` / `Editor` / `Preferences` / dialogs), which is why they live
//! on `App` rather than any one owner; the split is by concern.

use palantir::Ui;

use crate::gui::app::App;

pub(crate) mod edit;
pub(crate) mod file;
pub(crate) mod prefs;
pub(crate) mod run;

use edit::EditCommand;
use file::FileCommand;
use prefs::PrefsCommand;
use run::RunCommand;

use crate::gui::app::PendingTransition;

/// A command a UI surface (the menu bar, the graph toolbar, the Preferences
/// tab, a node's G-badge, an inline path-picker) hands to [`App`]. The producing
/// UI never touches the document / `Theme` / runtime services directly.
///
/// Everything here needs `&mut App`, a blocking dialog, or both, so none of it
/// can run during the pass that raises it. A UI action that only rearranges
/// panes is not one of these — it is a
/// [`DockOp`](crate::core::document::dock::DockOp) on the frame's queue.
#[derive(Clone, Debug)]
pub(crate) enum AppCommand {
    /// Document file lifecycle — [`file`](mod@file).
    File(FileCommand),
    /// Graph execution + worker event loop — [`run`].
    Run(RunCommand),
    /// Preferences edits — [`prefs`].
    Prefs(PrefsCommand),
    /// Node edits raised via a dialog — [`edit`].
    Edit(EditCommand),
    /// Quit the app, through the unsaved-changes prompt.
    Quit,
}

impl App {
    /// Dispatch a command after the editor has finished authoring its pass.
    pub(super) fn handle_command(&mut self, ui: &mut Ui, command: AppCommand) {
        match command {
            AppCommand::File(c) => self.handle_file(c),
            AppCommand::Run(c) => self.handle_run(c),
            AppCommand::Prefs(c) => self.handle_prefs(ui, c),
            AppCommand::Edit(c) => self.handle_edit(c),
            AppCommand::Quit => self.guard_discard(PendingTransition::Quit),
        }
    }
}
