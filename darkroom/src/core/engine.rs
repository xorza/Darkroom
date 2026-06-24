//! The runtime services shared by every frontend: the function library,
//! the evaluation worker, and the scripting-over-TCP host. `App` (GUI) and
//! `Session` (tui/headless) each own one and add their own document +
//! orchestration on top — so the worker/script construction and the
//! drain/run primitives live here once instead of in both shells.

use std::path::Path;

use scenarium::prelude::{Graph, OutputCache};

use crate::core::func_lib::{SharedFuncLib, runtime_codec_registry, runtime_func_lib};
use crate::core::io::cache::prepare_document_cache_root;
use crate::core::script::{ScriptConfig, ScriptHost, ScriptMessage};
use crate::core::wake::Wake;
use crate::core::worker::{ValueRequest, WorkerBridge, WorkerEvent};

#[derive(Debug)]
pub(crate) struct Engine {
    /// The shared runtime library. The GUI's promote/publish commands swap a
    /// grown copy into the cell; the worker (re-snapshots each run) and any
    /// running script executor observe it on their next `load`. See
    /// [`SharedFuncLib`].
    pub(crate) func_lib: SharedFuncLib,
    worker: WorkerBridge,
    /// `Some` only when `--script-tcp` bound a listener.
    script: Option<ScriptHost>,
}

impl Engine {
    /// Assemble the func lib (builtins + the on-disk subgraph library), spin
    /// up the evaluation worker, and start the script host (a no-op `None`
    /// unless `script_cfg` enabled a listener). The worker + script host are
    /// both woken through `wake`.
    pub(crate) fn new(script_cfg: &ScriptConfig, wake: Wake) -> Self {
        let func_lib = runtime_func_lib();
        let worker = WorkerBridge::new(wake.clone());
        // Install the codec registry up front (memory-only until a document has a
        // path); `set_document_cache` repoints the store root as documents open.
        worker.set_output_cache(OutputCache::new(runtime_codec_registry(), None));
        let script = ScriptHost::start(script_cfg, func_lib.clone(), wake);
        Self {
            func_lib,
            worker,
            script,
        }
    }

    /// Point the content-addressed cache at `doc_path`'s project-local store
    /// (`<stem>.darkroom-cache/` beside the file), so `persist` nodes reload across
    /// sessions. `None` (an unsaved document) is memory-only. Explicit-path cache
    /// nodes are unaffected — they always use their own path.
    pub(crate) fn set_document_cache(&self, doc_path: Option<&Path>) {
        let root = doc_path.map(prepare_document_cache_root);
        self.worker
            .set_output_cache(OutputCache::new(runtime_codec_registry(), root));
    }

    /// Send `graph` to the worker for one evaluation (paired with the
    /// startup func lib). Results arrive via [`Self::drain_worker`].
    pub(crate) fn run_once(&self, graph: Graph) {
        self.worker.run_once(graph, self.func_lib.load_full());
    }

    /// Request cancellation of the in-flight run (coarse — the running node
    /// finishes, nothing further is scheduled).
    pub(crate) fn cancel_run(&self) {
        self.worker.cancel_run();
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
