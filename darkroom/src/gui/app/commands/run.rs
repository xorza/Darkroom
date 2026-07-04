//! Graph execution + the worker event loop. These bridge the editor's
//! document (the graph to evaluate) and run-state with the engine's worker,
//! plus the `events_running` flag `App` mirrors so the toolbar toggle can't
//! lag the worker's real state.

use crate::gui::app::App;

/// Graph execution + the worker event loop. Handled by [`App::handle_run`].
#[derive(Clone, Copy, Debug)]
pub(crate) enum RunCommand {
    /// Evaluate the graph once on the worker.
    Once,
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
            RunCommand::Cancel => self.engine.cancel_run(),
            RunCommand::StartEvents => self.start_events(),
            RunCommand::StopEvents => self.stop_events(),
        }
    }

    /// Send the whole document graph to the worker and execute its
    /// terminals once. Opens a fresh value-cache epoch first — a re-run
    /// invalidates last run's per-node values and tags the replies the open
    /// panels request. The worker's `Update` tears down any running event
    /// loop, so the toggle drops in lockstep.
    pub(crate) fn run_graph(&mut self) {
        self.editor.run_state.begin_run();
        self.engine.run_once(self.editor.document.graph.clone());
        self.events_running = false;
    }

    /// Start the worker's event loop on the current graph: emitter events
    /// fire their subscribers until stopped.
    fn start_events(&mut self) {
        self.engine
            .start_event_loop(self.editor.document.graph.clone());
        self.events_running = true;
    }

    /// Stop the worker's event loop.
    fn stop_events(&mut self) {
        self.engine.stop_event_loop();
        self.events_running = false;
    }
}
