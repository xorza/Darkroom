//! Publishing graphs into the shared library: the thin orchestration
//! (file dialogs, marking the document dirty) over the pure resolution +
//! mutation in [`crate::core::edit::publish`], routed through
//! [`Engine::edit_library`](crate::core::engine::Engine::edit_library) —
//! which owns persisting the library file and refreshing the worker's
//! library snapshot.

use scenarium::NodeId;

use crate::core::edit::publish;
use crate::core::io::persistence;
use crate::gui::app::App;
use crate::gui::dialogs;

/// Publishing graphs into the shared library. Handled by
/// [`App::handle_graph`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum GraphCommand {
    /// Export the active graph (plus its local-graph dependencies) to a
    /// file. No-op when the active tab isn't a graph.
    Export,
    /// Import a graph bundle from a file into the current document.
    Import,
    /// Publish a copy of the active graph into the shared library, so it
    /// can be instanced as `Linked` anywhere. No-op off a graph.
    Promote,
    /// Publish a node's local graph to the library (the G-badge
    /// "Publish" action): update in place when linked, else create + link.
    PublishNode { node_id: NodeId },
}

impl App {
    pub(crate) fn handle_graph(&mut self, command: GraphCommand) {
        match command {
            GraphCommand::Export => self.export_active_graph(),
            GraphCommand::Import => self.import_graph(),
            GraphCommand::Promote => self.promote_active_graph(),
            GraphCommand::PublishNode { node_id } => self.publish_node_graph(node_id),
        }
    }

    /// Export a graph to a file (its interior `Graph` carries any
    /// nested graphs along). A selected graph-instance node
    /// wins; otherwise, when the active tab is itself a graph, that
    /// open graph is exported. No-op when neither resolves.
    fn export_active_graph(&mut self) {
        let library = self.engine.library().clone();
        let Some(graph) = publish::graph_to_export(&self.editor.document, &library) else {
            self.engine
                .status
                .error("graph export: no graph selected or open".into());
            return;
        };
        if let Some(path) = dialogs::pick_graph_save_path(self.current_path.as_deref()) {
            match persistence::export_graph(graph, &path) {
                Ok(()) => self.engine.status.error = None,
                Err(err) => self
                    .engine
                    .status
                    .error(format!("graph export failed: {err:#}")),
            }
        }
    }

    /// Import a graph from a file as a local graph in the current
    /// document. The import is a copy with a fresh id; nothing is
    /// instantiated and the undo stack is untouched (existing history
    /// references no imported graph, so it stays valid).
    fn import_graph(&mut self) {
        let Some(path) = dialogs::pick_graph_open_path(self.current_path.as_deref()) else {
            return;
        };
        match persistence::import_graph(&path) {
            Ok(graph) => {
                self.editor.import_graph(graph);
                self.engine.status.error = None;
            }
            Err(err) => self
                .engine
                .status
                .error(format!("graph import failed: {err:#}")),
        }
    }

    /// Promote a copy of the active/selected graph into the shared
    /// library and persist it, so it can be instanced as `Linked` anywhere.
    /// On success the source local graph's `origin` is re-pointed at the new
    /// entry (see [`publish::promote_to_library`]). No-op when nothing
    /// resolves.
    fn promote_active_graph(&mut self) {
        let document = &mut self.editor.document;
        if self
            .engine
            .edit_library(|lib| publish::promote_to_library(document, lib))
        {
            // Re-points the local graph's `origin` in the document — an
            // unsaved change (may over-flag when the link already existed).
            // The status outcome is owned by `edit_library`.
            self.editor.dirty = true;
        } else {
            self.engine
                .status
                .error("graph promote: no graph selected or open".into());
        }
    }

    /// Publish a node's local graph to the shared library (the
    /// G-badge "Publish" action): update in place when linked to a library
    /// graph, else create a fresh entry and link it (see
    /// [`publish::publish_local_graph`]). Non-undoable (library + disk only).
    fn publish_node_graph(&mut self, node_id: NodeId) {
        // The G-badge that raises this only exists on the canvas, so a
        // graph tab is always active here; bail otherwise.
        let Some(target) = self.editor.document.active_target() else {
            return;
        };
        let document = &mut self.editor.document;
        if self
            .engine
            .edit_library(|lib| publish::publish_local_graph(document, lib, target, node_id))
        {
            // Publishing a fresh entry re-points the local graph's `origin`
            // in the document — an unsaved change (an update-in-place
            // publish touches only the library, so this may over-flag).
            // The status outcome is owned by `edit_library`.
            self.editor.dirty = true;
        } else {
            self.engine
                .status
                .error("graph publish: node is not a local graph".into());
        }
    }
}
