//! Node edits that need a blocking dialog before applying — currently the
//! inline `FsPath` const-input picker. The dialog opens after UI authoring,
//! then the chosen paths land as an ordinary undoable `SetInput` edit.

use crate::core::edit::relayout::Relayout;
use crate::gui::app::App;
use crate::gui::pane::graph::node::port_row::PathPick;

/// Node edits that need a dialog before applying. Applied by
/// [`EditCommand::apply`].
#[derive(Clone, Debug)]
pub(crate) enum EditCommand {
    /// Open a file dialog (filtered by the pick's picker config) for a
    /// node's `FsPath` const input, applying the chosen paths as a `SetInput`
    /// edit. Raised by the inline pick button, from the port row's value cell.
    PickInputPath(PathPick),
}

impl EditCommand {
    #[must_use]
    pub(super) fn apply(self, app: &mut App) -> Relayout {
        match self {
            EditCommand::PickInputPath(pick) => app.pick_input_path(&pick),
        }
    }
}
