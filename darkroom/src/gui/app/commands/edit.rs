//! Node edits that need a blocking dialog before applying — currently the
//! inline `FsPath` const-input picker. The dialog opens after UI authoring,
//! then the chosen paths land as an ordinary undoable `SetInput` edit.

use scenarium::Binding;
use scenarium::FsPathMode;
use scenarium::StaticValue;

use crate::core::edit::intent::types::GraphIntent;
use crate::gui::app::App;
use crate::gui::dialogs;
use crate::gui::pane::graph::frame::prepass::PathPick;

/// Node edits that need a dialog before applying. Handled by
/// [`App::handle_edit`].
#[derive(Clone, Debug)]
pub(crate) enum EditCommand {
    /// Open a file dialog (filtered by the pick's picker config) for a
    /// node's `FsPath` const input, applying the chosen paths as a `SetInput`
    /// edit. Raised by the inline pick button (see `gui::pane::graph::frame::prepass::emit_path_picks`,
    /// which produces the [`PathPick`]).
    PickInputPath(PathPick),
}

impl App {
    pub(super) fn handle_edit(&mut self, command: EditCommand) {
        match command {
            EditCommand::PickInputPath(pick) => self.pick_input_path(pick),
        }
    }

    /// Open a file dialog for a node's `FsPath` const input and, if the
    /// user makes a selection, apply the chosen paths as a `SetInput` edit. Runs after
    /// authoring, so it goes through `Editor::apply_edit` rather than the
    /// frame's intent drain.
    fn pick_input_path(&mut self, pick: PathPick) {
        let extensions: Vec<&str> = pick.config.extensions.iter().map(String::as_str).collect();
        let value = match pick.config.mode {
            FsPathMode::ExistingFile => dialogs::pick_existing_file(&extensions)
                .map(|path| StaticValue::FsPath(path.to_string_lossy().into_owned())),
            FsPathMode::ExistingFiles => dialogs::pick_existing_files(&extensions).map(|paths| {
                StaticValue::FsPaths(
                    paths
                        .into_iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                )
            }),
            FsPathMode::NewFile => dialogs::pick_new_file(&extensions)
                .map(|path| StaticValue::FsPath(path.to_string_lossy().into_owned())),
            FsPathMode::Directory => dialogs::pick_directory()
                .map(|path| StaticValue::FsPath(path.to_string_lossy().into_owned())),
        };
        let Some(value) = value else {
            return;
        };
        self.editor.apply_edit(
            &mut self.open,
            GraphIntent::SetInput {
                input: pick.port,
                to: Some(Binding::Const(value)),
            },
        );
    }
}
