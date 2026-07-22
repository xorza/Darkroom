//! Graph-template import/export and graph-library publishing orchestration.

use scenarium::NodeId;

use crate::core::edit::publish;
use crate::core::io::graph_template;
use crate::gui::app::App;
use crate::gui::dialogs;

/// Publishing graphs into the shared library. Handled by
/// [`App::handle_graph`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum GraphCommand {
    /// Export the active graph (plus its local-graph dependencies) to a
    /// file. No-op when the active tab isn't a graph.
    ExportGraphTemplate,
    /// Import a graph template as a new persistent graph-library entry.
    ImportGraphTemplateIntoLibrary,
    /// Publish a node's local graph to the library (the G-badge
    /// "Publish" action): update in place when linked, else create + link.
    PublishGraphToLibrary { node_id: NodeId },
}

impl App {
    pub(crate) fn handle_graph(&mut self, command: GraphCommand) {
        match command {
            GraphCommand::ExportGraphTemplate => self.export_active_graph_template(),
            GraphCommand::ImportGraphTemplateIntoLibrary => {
                self.import_graph_template_into_library();
            }
            GraphCommand::PublishGraphToLibrary { node_id } => {
                self.publish_graph_to_library(node_id);
            }
        }
    }

    /// Export a graph to a file (its interior `Graph` carries any
    /// nested graphs along). A selected graph-instance node
    /// wins; otherwise, when the active tab is itself a graph, that
    /// open graph is exported. No-op when neither resolves.
    fn export_active_graph_template(&mut self) {
        let library = self.workspace.runtime.library.published.load();
        let Some(graph) =
            publish::graph_template_to_export(&self.workspace.open.document, &library)
        else {
            self.workspace
                .runtime
                .status
                .error("graph export: no graph selected or open".into());
            return;
        };
        if let Some(path) =
            dialogs::pick_graph_template_save_path(self.workspace.open.path.as_deref())
        {
            match graph_template::save(graph, &path) {
                Ok(()) => self.workspace.runtime.status.error = None,
                Err(err) => self
                    .workspace
                    .runtime
                    .status
                    .error(format!("graph export failed: {err:#}")),
            }
        }
    }

    fn import_graph_template_into_library(&mut self) {
        let Some(path) =
            dialogs::pick_graph_template_open_path(self.workspace.open.path.as_deref())
        else {
            return;
        };
        match graph_template::load(&path) {
            Ok(graph) => {
                let graph = graph.fresh_copy();
                self.workspace.runtime.import_template(graph);
            }
            Err(error) => self
                .workspace
                .runtime
                .status
                .error(format!("graph-library import failed: {error:#}")),
        }
    }

    /// Publish a node's local graph to the shared library (the
    /// G-badge "Publish" action): update in place when linked to a library
    /// graph, else create a fresh entry and link it. Non-undoable.
    fn publish_graph_to_library(&mut self, node_id: NodeId) {
        // The G-badge that raises this only exists on the canvas, so a
        // graph tab is always active here; bail otherwise.
        let Some(target) = self.workspace.open.document.active_target() else {
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
