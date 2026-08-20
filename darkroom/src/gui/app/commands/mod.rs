//! [`AppCommand`] handling: file / run / preferences / edit side effects, plus
//! the quit request. Commands are produced by action input, which Palantir
//! exposes only to the first record pass, so handlers can run directly after
//! authoring.
//!
//! [`AppCommand::apply`] is a thin dispatcher — each top-level command group
//! resolves to its own enum's `apply` (`file`, `run`, `prefs`, `edit`); `Quit`
//! carries no payload and resolves here. Each `apply` reads as the command's
//! own behaviour, but the work it drives is cross-subsystem coordination (it
//! bridges the open document / `RuntimeHost` / `Editor` / `Preferences` /
//! dialogs), so the methods it calls belong to `App` rather than any one
//! owner; only the dispatch splits by concern.

use palantir::Ui;

use crate::gui::app::App;
use crate::gui::relayout::Relayout;

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
/// [`DockOp`](crate::core::document::dock::dock_op::DockOp) on the frame's queue.
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

impl AppCommand {
    /// Dispatch a command after the editor has finished authoring its pass.
    ///
    /// Returns whether the command stranded the canvas's cached geometry.
    /// Only an edit can — the rest touch state no canvas measures against —
    /// but it is reported rather than requested here so that `App::frame`
    /// stays the one place in the app that asks for a relayout.
    #[must_use]
    pub(super) fn apply(self, app: &mut App, ui: &mut Ui) -> Relayout {
        match self {
            AppCommand::File(c) => {
                c.apply(app);
                Relayout::NotNeeded
            }
            AppCommand::Run(c) => {
                c.apply(app);
                Relayout::NotNeeded
            }
            AppCommand::Prefs(c) => {
                c.apply(app, ui);
                Relayout::NotNeeded
            }
            AppCommand::Edit(c) => c.apply(app),
            AppCommand::Quit => {
                app.guard_discard(PendingTransition::Quit);
                Relayout::NotNeeded
            }
        }
    }
}
