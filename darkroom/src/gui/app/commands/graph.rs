//! Graph-library publishing orchestration.

use scenarium::NodeId;

use crate::gui::app::App;

/// Publishing graphs into the shared library. Handled by
/// [`App::handle_graph`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum GraphCommand {
    /// Publish a node's local graph to the library (the G-badge
    /// "Publish" action): update in place when linked, else create + link.
    PublishGraphToLibrary { node_id: NodeId },
}

impl App {
    pub(super) fn handle_graph(&mut self, command: GraphCommand) {
        match command {
            GraphCommand::PublishGraphToLibrary { node_id } => {
                self.publish_graph_to_library(node_id);
            }
        }
    }

    /// Publish a node's local graph to the shared library (the
    /// G-badge "Publish" action): update in place when linked to a library
    /// graph, else create a fresh entry and link it. Non-undoable.
    fn publish_graph_to_library(&mut self, node_id: NodeId) {
        // The G-badge that raises this only exists on the canvas, so a
        // graph tab is always active here; bail otherwise.
        let Some(target) = self.workspace.open.document.focused_target() else {
            return;
        };
        let document = &mut self.workspace.open.document;
        if self
            .workspace
            .runtime
            .publish_graph_to_library(document, target, node_id)
        {
            // Publishing a fresh entry re-points the local graph's `origin`
            // in the document — an unsaved change (an update-in-place
            // publish touches only the library, so this may over-flag).
            // The status outcome is owned by the runtime host.
            self.editor.dirty = true;
        } else {
            self.workspace
                .runtime
                .status
                .error("graph publish: node is not a local graph".into());
        }
    }
}
