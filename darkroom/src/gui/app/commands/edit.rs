//! Node edits that need a blocking dialog before applying — currently the
//! inline `FsPath` const-input picker. The dialog opens after UI authoring,
//! then the chosen paths land as an ordinary undoable `SetInput` edit.

use palantir::Ui;
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
    pub(super) fn handle_edit(&mut self, ui: &mut Ui, command: EditCommand) {
        match command {
            EditCommand::PickInputPath(pick) => self.pick_input_path(ui, pick),
        }
    }

    /// Open a file dialog for a node's `FsPath` const input and, if the
    /// user makes a selection, apply the chosen paths as a `SetInput` edit. Runs after
    /// authoring, so it goes through `Editor::apply_edit` rather than the
    /// frame's intent drain.
    ///
    /// Spends the edit's relayout here rather than handing it back: this runs
    /// past the point where `Editor::frame` has already spent the frame's own,
    /// so an edit that resizes a node body would otherwise leave the canvas
    /// geometry stale until something else happened to ask for a pass.
    fn pick_input_path(&mut self, ui: &mut Ui, pick: PathPick) {
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
        let needs_relayout = self.editor.apply_edit(
            &mut self.open,
            GraphIntent::SetInput {
                input: pick.port,
                to: Some(Binding::Const(value)),
            },
        );
        if needs_relayout {
            ui.request_relayout();
        }
    }
}
