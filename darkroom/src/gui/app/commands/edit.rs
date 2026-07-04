//! Node edits that need a blocking dialog before applying — currently the
//! inline `FsPath` const-input picker. The dialog runs outside the record,
//! then the chosen path lands as an ordinary undoable `SetInput` edit.

use std::sync::Arc;

use scenarium::data::{FsPathConfig, StaticValue};
use scenarium::graph::{Binding, NodeId};

use crate::core::edit::intent::Intent;
use crate::gui::app::App;
use crate::gui::app::EditCommand;
use crate::gui::dialogs;

impl App {
    pub(crate) fn handle_edit(&mut self, command: EditCommand) {
        match command {
            EditCommand::PickInputPath {
                node_id,
                port_idx,
                config,
            } => self.pick_input_path(node_id, port_idx, config),
        }
    }

    /// Open a file dialog for a node's `FsPath` const input and, if the
    /// user picks one, apply the chosen path as a `SetInput` edit. Runs
    /// outside the record (blocking dialog), so it goes through
    /// `Editor::apply_edit` rather than the frame's intent drain.
    fn pick_input_path(&mut self, node_id: NodeId, port_idx: usize, config: Arc<FsPathConfig>) {
        let Some(path) = dialogs::pick_path(&config) else {
            return;
        };
        let value = StaticValue::FsPath(path.to_string_lossy().into_owned());
        let library = self.engine.library.load();
        self.editor.apply_edit(
            Intent::SetInput {
                node_id,
                input_idx: port_idx,
                to: Binding::Const(value),
            },
            &library,
        );
    }
}
