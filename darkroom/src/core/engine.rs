//! The runtime services shared by every frontend: the function library,
//! the evaluation worker, and the scripting-over-TCP host. `App` (GUI) and
//! `Session` (tui/headless) each own one and add their own document +
//! orchestration on top — so the worker/script construction and the
//! drain/run primitives live here once instead of in both shells.

use std::path::Path;
use std::sync::Arc;

use scenarium::execution::compile::{CompiledGraph, Compiler};
use scenarium::execution::disk_store::DiskStore;
use scenarium::execution::stats::FlattenMap;
use scenarium::graph::{Graph, NodeId};

use crate::core::io::cache::prepare_document_cache_root;
use crate::core::library::{SharedLibrary, runtime_func_lib};
use crate::core::script::{ScriptConfig, ScriptHost, ScriptMessage};
use crate::core::status::StatusLog;
use crate::core::wake::Wake;
use crate::core::worker::{ValueRequest, WorkerBridge, WorkerEvent};

#[derive(Debug)]
pub(crate) struct Engine {
    /// The shared runtime library. The GUI's promote/publish commands swap a
    /// grown copy into the cell; the worker (re-snapshots each run) and any
    /// running script executor observe it on their next `load`. See
    /// [`SharedLibrary`].
    pub(crate) library: SharedLibrary,
    worker: WorkerBridge,
    /// Long-lived so the flatten scratch is reused across compiles instead of
    /// reallocated per run.
    compiler: Compiler,
    /// The flatten map of the last program sent to the worker — the worker's
    /// stats are keyed by flat ids and carry no map of their own (the host
    /// compiled, so it already has it). Frontends project stats through this.
    pub(crate) flatten_map: Arc<FlattenMap>,
    /// The shared user-facing outcome log: compile failures report here
    /// (from [`Self::compile`]); frontends add their own outcomes (run
    /// results, file ops) and render it — the TUI as a rolling history, the
    /// GUI as the sticky error slot in the status bar.
    pub(crate) status: StatusLog,
    /// `Some` only when `--script-tcp` bound a listener.
    script: Option<ScriptHost>,
}

impl Engine {
    /// Assemble the func lib (builtins + the on-disk subgraph library), spin
    /// up the evaluation worker, and start the script host (a no-op `None`
    /// unless `script_cfg` enabled a listener). The worker + script host are
    /// both woken through `wake`.
    pub(crate) fn new(script_cfg: &ScriptConfig, wake: Wake) -> Self {
        let library = runtime_func_lib();
        let worker = WorkerBridge::new(wake.clone());
        // Install the cache up front (memory-only until a document has a path);
        // its codecs come from the library snapshot, and `set_document_cache`
        // repoints the store root as documents open.
        worker.set_disk_store(DiskStore::new(library.load_full(), None));
        let script = ScriptHost::start(script_cfg, library.clone(), wake);
        Self {
            library,
            worker,
            compiler: Compiler::default(),
            flatten_map: Arc::default(),
            status: StatusLog::default(),
            script,
        }
    }

    /// Compile `graph` against the current library and record the artifact's
    /// flatten map as the engine's current one — every send below installs its
    /// artifact on the worker, so the map here always mirrors the program the
    /// worker's next stats come from. A failure is reported to [`Self::status`]
    /// and returns `None` (nothing sent, worker untouched); a success clears
    /// the sticky error.
    fn compile(&mut self, graph: &Graph) -> Option<CompiledGraph> {
        match self.compiler.compile(graph, &self.library.load()) {
            Ok(compiled) => {
                self.flatten_map = compiled.flatten_map.clone();
                self.status.error = None;
                Some(compiled)
            }
            Err(e) => {
                self.status.error(format!("compile failed: {e}"));
                None
            }
        }
    }

    /// Point the content-addressed cache at `doc_path`'s project-local store
    /// (`<stem>.darkroom-cache/` beside the file), so disk-backed (`Disk`/`Both`)
    /// nodes reload across sessions. `None` (an unsaved document) is memory-only. Explicit-path cache
    /// nodes are unaffected — they always use their own path.
    pub(crate) fn set_document_cache(&self, doc_path: Option<&Path>) {
        let root = doc_path.map(prepare_document_cache_root);
        self.worker
            .set_disk_store(DiskStore::new(self.library.load_full(), root));
    }

    /// Compile `graph` against the current library and send it to the worker
    /// for one evaluation. Returns whether it was sent — a compile failure is
    /// reported to [`Self::status`] synchronously and nothing reaches the
    /// worker. Results arrive via [`Self::drain_worker`].
    pub(crate) fn run_once(&mut self, graph: &Graph) -> bool {
        let Some(compiled) = self.compile(graph) else {
            return false;
        };
        self.worker.run_once(compiled);
        true
    }

    /// Compile `graph` and evaluate only `node_id`'s upstream cone, keeping
    /// its outputs resident for the preview fetch ("run to this node").
    /// Returns whether it was sent — a compile failure is reported to
    /// [`Self::status`] and nothing reaches the worker. Results arrive via
    /// [`Self::drain_worker`].
    pub(crate) fn run_node(&mut self, graph: &Graph, node_id: NodeId) -> bool {
        let Some(compiled) = self.compile(graph) else {
            return false;
        };
        self.worker.run_node(compiled, node_id);
        true
    }

    /// Persist resident caches to disk without running the graph — e.g. after a node's
    /// disk-cache toggle, so its RAM value reaches disk immediately. The recompile
    /// carries the just-toggled cache mode to the worker; a compile failure is
    /// reported to [`Self::status`] and nothing is sent.
    pub(crate) fn save_caches(&mut self, graph: &Graph) {
        if let Some(compiled) = self.compile(graph) {
            self.worker.save_caches(compiled);
        }
    }

    /// Request cancellation of the in-flight run (coarse — the running node
    /// finishes, nothing further is scheduled).
    pub(crate) fn cancel_run(&self) {
        self.worker.cancel_run();
    }

    /// Start the event loop on `graph` (compiles + loads it, then fires
    /// events). The worker's `Update` tears down any prior loop first.
    /// Returns whether it was sent — a compile failure is reported to
    /// [`Self::status`] and the loop's running state is untouched.
    pub(crate) fn start_event_loop(&mut self, graph: &Graph) -> bool {
        let Some(compiled) = self.compile(graph) else {
            return false;
        };
        self.worker.start_event_loop(compiled);
        true
    }

    /// Stop the event loop.
    pub(crate) fn stop_event_loop(&self) {
        self.worker.stop_event_loop();
    }

    /// Non-blocking drain of worker results posted since the last frame.
    pub(crate) fn drain_worker(&self) -> impl Iterator<Item = WorkerEvent> + '_ {
        self.worker.drain()
    }

    /// Non-blocking drain of everything scripts have pushed since the last
    /// frame (empty when no listener is running).
    pub(crate) fn drain_script(&mut self) -> Vec<ScriptMessage> {
        match &mut self.script {
            Some(script) => script.drain(),
            None => Vec::new(),
        }
    }

    /// Ask the worker for one node's computed input/output values (GUI
    /// inspector panels). The reply lands on a later [`Self::drain_worker`].
    pub(crate) fn request_argument_values(&self, request: ValueRequest) {
        self.worker.request_argument_values(request);
    }
}
