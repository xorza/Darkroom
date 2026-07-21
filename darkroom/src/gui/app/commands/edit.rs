//! Node edits that need a blocking dialog before applying — currently the
//! inline `FsPath` const-input picker. The dialog opens after UI authoring,
//! then the chosen path lands as an ordinary undoable `SetInput` edit.

use scenarium::Binding;
use scenarium::StaticValue;

use crate::core::edit::intent::types::Intent;
use crate::gui::app::App;
use crate::gui::dialogs;
use crate::gui::node::prepass::PathPickRequest;

/// Node edits that need a dialog before applying. Handled by
/// [`App::handle_edit`].
#[derive(Clone, Debug)]
pub(crate) enum EditCommand {
    /// Open a file dialog (filtered by the request's picker config) for a
    /// node's `FsPath` const input, applying the chosen path as a `SetInput`
    /// edit. Raised by the inline pick button (see `gui::node::prepass::emit_path_picks`,
    /// which produces the [`PathPickRequest`]).
    PickInputPath(PathPickRequest),
}

impl App {
    pub(crate) fn handle_edit(&mut self, command: EditCommand) {
        match command {
            EditCommand::PickInputPath(req) => self.pick_input_path(req),
        }
    }

    /// Open a file dialog for a node's `FsPath` const input and, if the
    /// user picks one, apply the chosen path as a `SetInput` edit. Runs after
    /// authoring, so it goes through `Editor::apply_edit` rather than the
    /// frame's intent drain.
    fn pick_input_path(&mut self, req: PathPickRequest) {
        let Some(path) = dialogs::pick_path(&req.config) else {
            return;
        };
        let value = StaticValue::FsPath(path.to_string_lossy().into_owned());
        let library = self.engine.library.current.clone();
        self.editor.apply_edit(
            Intent::SetInput {
                input: req.port,
                to: Some(Binding::Const(value)),
            },
            &library,
        );
    }
}
