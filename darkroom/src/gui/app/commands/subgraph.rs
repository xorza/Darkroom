//! Publishing subgraphs into the shared library: the thin orchestration
//! (file dialogs, library disk writes, marking the document dirty) over the
//! pure resolution + mutation in [`crate::core::edit::publish`].

use scenarium::graph::NodeId;

use crate::core::edit::publish;
use crate::core::io::{library, persistence};
use crate::gui::app::App;
use crate::gui::dialogs;

/// Publishing subgraphs into the shared library. Handled by
/// [`App::handle_subgraph`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum SubgraphCommand {
    /// Export the active subgraph (plus its local-def dependencies) to a
    /// file. No-op when the active tab isn't a subgraph.
    Export,
    /// Import a subgraph bundle from a file into the current document.
    Import,
    /// Publish a copy of the active subgraph into the shared library, so it
    /// can be instanced as `Linked` anywhere. No-op off a subgraph.
    Promote,
    /// Publish a node's local subgraph def to the library (the S-badge
    /// "Publish" action): update in place when linked, else create + link.
    PublishNode { node_id: NodeId },
}

impl App {
    pub(crate) fn handle_subgraph(&mut self, command: SubgraphCommand) {
        match command {
            SubgraphCommand::Export => self.export_active_subgraph(),
            SubgraphCommand::Import => self.import_subgraph(),
            SubgraphCommand::Promote => self.promote_active_subgraph(),
            SubgraphCommand::PublishNode { node_id } => self.publish_node_subgraph(node_id),
        }
    }

    /// Export a subgraph def to a file (its interior `Graph` carries any
    /// nested subgraph defs along). A selected subgraph-instance node
    /// wins; otherwise, when the active tab is itself a subgraph, that
    /// open subgraph is exported. No-op when neither resolves.
    fn export_active_subgraph(&mut self) {
        let library = self.engine.library.load();
        let Some(def) = publish::subgraph_to_export(&self.editor.document, &library) else {
            eprintln!("subgraph export: no subgraph selected or open");
            return;
        };
        if let Some(path) = dialogs::pick_save_path(self.current_path.as_deref()) {
            persistence::export_subgraph(def, &path);
        }
    }

    /// Import a subgraph def from a file as a local def in the current
    /// document. The import is a copy with a fresh id; nothing is
    /// instantiated and the undo stack is untouched (existing history
    /// references no imported def, so it stays valid).
    fn import_subgraph(&mut self) {
        let Some(path) = dialogs::pick_open_path(self.current_path.as_deref()) else {
            return;
        };
        if let Some(def) = persistence::import_subgraph(&path) {
            self.editor.import_subgraph(def);
        }
    }

    /// Promote a copy of the active/selected subgraph into the shared
    /// library and persist it, so it can be instanced as `Linked` anywhere.
    /// On success the source local def's `origin` is re-pointed at the new
    /// entry (see [`publish::promote_to_library`]). No-op when nothing
    /// resolves.
    fn promote_active_subgraph(&mut self) {
        if publish::promote_to_library(&mut self.editor.document, &self.engine.library) {
            // Re-points the local def's `origin` in the document — an
            // unsaved change (may over-flag when the link already existed).
            self.editor.dirty = true;
            library::save_library(self.engine.library.load().subgraphs.iter());
        } else {
            eprintln!("subgraph promote: no subgraph selected or open");
        }
    }

    /// Publish a node's local subgraph def to the shared library (the
    /// S-badge "Publish" action): update in place when linked to a library
    /// def, else create a fresh entry and link it (see
    /// [`publish::publish_local_def`]). Non-undoable (library + disk only).
    fn publish_node_subgraph(&mut self, node_id: NodeId) {
        // The S-badge that raises this only exists on the canvas, so a
        // graph tab is always active here; bail otherwise.
        let Some(target) = self.editor.document.active_target() else {
            return;
        };
        if publish::publish_local_def(
            &mut self.editor.document,
            &self.engine.library,
            target,
            node_id,
        ) {
            // Publishing a fresh entry re-points the local def's `origin`
            // in the document — an unsaved change (an update-in-place
            // publish touches only the library, so this may over-flag).
            self.editor.dirty = true;
            library::save_library(self.engine.library.load().subgraphs.iter());
        } else {
            eprintln!("subgraph publish: node is not a local subgraph");
        }
    }
}
