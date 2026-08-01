//! Graph execution + the worker event loop. Commands only request work;
//! worker status reports drive the toolbar's execution and loop state.

use scenarium::NodeId;

use crate::gui::app::App;

/// Graph execution + the worker event loop. Handled by [`App::handle_run`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum RunCommand {
    /// Evaluate the graph once on the worker.
    Once,
    /// Evaluate one node's upstream cone and deliver its outputs.
    Node(NodeId),
    /// Remove one authored node's compiled runtime-cache cone from RAM and disk.
    EvictCache(NodeId),
    /// Request cancellation of the in-flight run.
    Cancel,
    /// Start the worker's event loop (emitter events → run subscribers).
    StartEvents,
    /// Stop the worker's event loop.
    StopEvents,
}

impl App {
    pub(super) fn handle_run(&mut self, command: RunCommand) {
        match command {
            RunCommand::Once => self.run_graph(),
            RunCommand::Node(node_id) => self.run_node(node_id),
            RunCommand::EvictCache(node_id) => self.evict_cache(node_id),
            RunCommand::Cancel => self.runtime.cancel_run(),
            RunCommand::StartEvents => self.start_events(),
            RunCommand::StopEvents => self.stop_events(),
        }
    }

    /// Compile the document graph and execute its sinks once on the
    /// worker. A compile error is reported to the engine's status log
    /// synchronously — no run starts, so the prior run's status stays
    /// untouched. Worker status reports acknowledge actual execution and
    /// event-loop transitions.
    pub(crate) fn run_graph(&mut self) {
        self.runtime.run_once(&self.session.open.document.graph);
    }

    /// Like [`Self::run_graph`], but seeds the run at one node: only its
    /// upstream cone executes and its outputs are delivered.
    fn run_node(&mut self, node_id: NodeId) {
        // A node inside a local definition has no enclosing instance path,
        // so no execution seed resolves. The UI gates the play chip and the
        // menu action on `NodeCtx::runnable`, which is false there —
        // reaching this is a gating bug, not user input, so refuse rather
        // than kill the editor from a live command handler. Tested against
        // the *node's* graph, not the focused pane's: with several graph
        // panes open, a root node's chip stays valid while focus sits
        // elsewhere.
        if self.session.open.document.graph.find(node_id).is_none() {
            debug_assert!(false, "run-node reached for a node outside the root graph");
            return;
        }
        self.runtime
            .run_node(&self.session.open.document.graph, node_id);
    }

    fn evict_cache(&mut self, node_id: NodeId) {
        if self
            .runtime
            .evict_cache(&self.session.open.document.graph, node_id)
        {
            self.run_state.clear_cache_projections();
        }
    }

    /// Start the worker's event loop on the current graph: emitter events
    /// fire their subscribers until stopped. A compile error (reported to
    /// the engine's status log) leaves the loop's running state as it was —
    /// nothing reached the worker.
    fn start_events(&mut self) {
        self.runtime
            .start_event_loop(&self.session.open.document.graph);
    }

    /// Stop the worker's event loop.
    fn stop_events(&mut self) {
        self.runtime.stop_event_loop();
    }
}
