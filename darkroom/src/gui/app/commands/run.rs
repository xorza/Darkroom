//! Graph execution + the worker event loop. These bridge the editor's
//! document (the graph to evaluate) and run-state with the engine's worker,
//! plus the `events_running` flag `App` mirrors so the toolbar toggle can't
//! lag the worker's real state.

use scenarium::graph::NodeId;

use crate::gui::app::App;

/// Graph execution + the worker event loop. Handled by [`App::handle_run`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum RunCommand {
    /// Evaluate the graph once on the worker.
    Once,
    /// Evaluate one node's upstream cone on the worker, keeping its
    /// outputs resident for preview ("run to this node").
    Node(NodeId),
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
            RunCommand::Cancel => self.engine.cancel_run(),
            RunCommand::StartEvents => self.start_events(),
            RunCommand::StopEvents => self.stop_events(),
        }
    }

    /// Compile the document graph and execute its terminals once on the
    /// worker. A compile error surfaces here, synchronously — no run starts,
    /// so the prior run's values/status stay untouched. On success, opens a
    /// fresh value-cache epoch — a re-run invalidates last run's per-node
    /// values and tags the replies the open panels request. The worker's
    /// `Update` tears down any running event loop, so the toggle drops in
    /// lockstep.
    pub(crate) fn run_graph(&mut self) {
        if let Err(e) = self.engine.run_once(&self.editor.document.graph) {
            self.report_error(format!("compile failed: {e}"));
            return;
        }
        self.status_error = None;
        self.editor.run_state.begin_run();
        self.events_running = false;
    }

    /// Like [`Self::run_graph`], but seeds the run at one node: only its
    /// upstream cone executes, and its outputs stay resident for the
    /// preview value fetch. Same epoch and event-loop bookkeeping as a
    /// full run — a preview run is just a run.
    pub(crate) fn run_node(&mut self, node_id: NodeId) {
        if let Err(e) = self.engine.run_node(&self.editor.document.graph, node_id) {
            self.report_error(format!("compile failed: {e}"));
            return;
        }
        self.status_error = None;
        self.editor.run_state.begin_run();
        self.events_running = false;
    }

    /// Start the worker's event loop on the current graph: emitter events
    /// fire their subscribers until stopped. A compile error leaves the
    /// loop's running state as it was — nothing reached the worker.
    fn start_events(&mut self) {
        if let Err(e) = self.engine.start_event_loop(&self.editor.document.graph) {
            self.report_error(format!("compile failed: {e}"));
            return;
        }
        self.status_error = None;
        self.events_running = true;
    }

    /// Stop the worker's event loop.
    fn stop_events(&mut self) {
        self.engine.stop_event_loop();
        self.events_running = false;
    }
}
