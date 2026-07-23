//! Graph execution + the worker event loop. Commands only request work;
//! worker lifecycle reports drive the toolbar's execution and loop state.

use scenarium::NodeId;

use crate::core::document::GraphRef;
use crate::gui::app::App;

/// Graph execution + the worker event loop. Handled by [`App::handle_run`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum RunCommand {
    /// Evaluate the graph once on the worker.
    Once,
    /// Evaluate one node's upstream cone and deliver its outputs.
    Node(NodeId),
    /// Remove one authored node's flattened runtime-cache cone from RAM and disk.
    EvictCache(NodeId),
    /// Request cancellation of the in-flight run.
    Cancel,
    /// Start the worker's event loop (emitter events → run subscribers).
    StartEvents,
    /// Stop the worker's event loop.
    StopEvents,
}

impl App {
    pub(crate) fn handle_run(&mut self, command: RunCommand) {
        match command {
            RunCommand::Once => self.run_graph(),
            RunCommand::Node(node_id) => self.run_node(node_id),
            RunCommand::EvictCache(node_id) => self.evict_cache(node_id),
            RunCommand::Cancel => self.workspace.runtime.cancel_run(),
            RunCommand::StartEvents => self.start_events(),
            RunCommand::StopEvents => self.stop_events(),
        }
    }

    /// Compile the document graph and execute its sinks once on the
    /// worker. A compile error is reported to the engine's status log
    /// synchronously — no run starts, so the prior run's status stays
    /// untouched. Worker lifecycle reports acknowledge actual execution and
    /// event-loop transitions.
    pub(crate) fn run_graph(&mut self) {
        self.workspace.run_once();
    }

    /// Like [`Self::run_graph`], but seeds the run at one node: only its
    /// upstream cone executes and its outputs are delivered.
    pub(crate) fn run_node(&mut self, node_id: NodeId) {
        if self.workspace.open.document.active_target() != Some(GraphRef::Main) {
            unimplemented!("run-node commands are only implemented for the main graph");
        }
        self.workspace.run_node(node_id);
    }

    fn evict_cache(&mut self, node_id: NodeId) {
        if self.workspace.evict_cache(node_id) {
            self.editor.run_state.clear_cache_projections();
        }
    }

    /// Start the worker's event loop on the current graph: emitter events
    /// fire their subscribers until stopped. A compile error (reported to
    /// the engine's status log) leaves the loop's running state as it was —
    /// nothing reached the worker.
    fn start_events(&mut self) {
        self.workspace.start_event_loop();
    }

    /// Stop the worker's event loop.
    fn stop_events(&mut self) {
        self.workspace.runtime.stop_event_loop();
    }
}
