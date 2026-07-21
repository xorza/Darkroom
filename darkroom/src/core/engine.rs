//! The runtime services shared by every frontend: the function library,
//! the evaluation worker, and the scripting-over-TCP host. `App` (GUI) and
//! `Session` (tui/headless) each own one and add their own document +
//! orchestration on top — so the worker/script construction and the
//! drain/run primitives live here once instead of in both shells.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use scenarium::DiskStore;
use scenarium::FlattenMap;
use scenarium::Library;
use scenarium::{Compilation, CompiledGraph, Compiler};
use scenarium::{Graph, NodeId};

use crate::core::io::cache::prepare_document_cache_root;
use crate::core::io::library::{load_library, save_library};
use crate::core::library::runtime_func_lib;
use crate::core::script::{ScriptConfig, ScriptHost, ScriptMessage};
use crate::core::status::StatusLog;
use crate::core::wake::Wake;
use crate::core::worker::{WorkerBridge, WorkerEvent};

#[derive(Debug)]
pub(crate) struct Engine {
    /// The runtime library (builtins + the on-disk graph library),
    /// assembled once at startup. Private so every edit goes through
    /// [`Self::edit_library`] — the one place that propagates a change to
    /// every copy that could otherwise go stale. Read via [`Self::library`].
    library: Arc<Library>,
    worker: WorkerBridge,
    /// The active disk-store root (`None` = memory-only),
    /// remembered so [`Self::edit_library`] can re-push the worker's
    /// [`DiskStore`] — which carries a library snapshot — without the caller
    /// re-supplying the document path.
    disk_root: Option<PathBuf>,
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
    /// Assemble the func lib (builtins + the on-disk graph library), spin
    /// up the evaluation worker, and start the script host (a no-op `None`
    /// unless `script_cfg` enabled a listener). The worker + script host are
    /// both woken through `wake`.
    pub(crate) fn new(script_cfg: &ScriptConfig, wake: Wake) -> Self {
        let mut library = runtime_func_lib();
        // A failed load degrades to an empty library instead of blocking
        // startup; `load_library` moved the file aside so nothing here can
        // overwrite it (reported below, once the status log exists).
        let load_error = match load_library() {
            Ok(graphs) => {
                for (id, graph) in graphs {
                    library.insert_graph(id, graph);
                }
                None
            }
            Err(err) => Some(err),
        };
        let library = Arc::new(library);
        let worker = WorkerBridge::new(wake.clone());
        let script = ScriptHost::start(script_cfg, library.clone(), wake);
        let mut engine = Self {
            library,
            worker,
            disk_root: None,
            compiler: Compiler::default(),
            flatten_map: Arc::default(),
            status: StatusLog::default(),
            script,
        };
        // Install the store up front (memory-only until a document has a
        // path); `set_document_cache` repoints the root as documents open.
        engine.refresh_disk_store();
        if let Some(err) = load_error {
            engine.status.error(format!("library load failed: {err:#}"));
        }
        engine
    }

    /// Read access to the runtime library. There is deliberately no mutable
    /// counterpart — edits go through [`Self::edit_library`], which owns the
    /// propagation a bare `&mut` would let callers forget.
    pub(crate) fn library(&self) -> &Arc<Library> {
        &self.library
    }

    /// The single mutation path for the runtime library. Applies `edit`
    /// (copy-on-write when the script host still holds the startup `Arc`)
    /// and, when it reports a change, propagates to every copy that could
    /// otherwise go stale: the graph library file on disk and the
    /// worker's [`DiskStore`] (which carries a library snapshot for its
    /// codec table). The script host's frozen startup snapshot is exempt by
    /// design — scripts only read the func table, which no edit touches
    /// (edits grow graphs).
    ///
    /// Owns the persist outcome in [`Self::status`]: a saved change clears
    /// the sticky error, a failed save reports — callers must not overwrite
    /// it with their own success signal.
    pub(crate) fn edit_library(&mut self, edit: impl FnOnce(&mut Library) -> bool) -> bool {
        let changed = edit(Arc::make_mut(&mut self.library));
        if changed {
            match save_library(self.library.graphs.iter()) {
                Ok(()) => self.status.error = None,
                Err(err) => self.status.error(format!("library save failed: {err:#}")),
            }
            self.refresh_disk_store();
        }
        changed
    }

    /// Push a fresh [`DiskStore`] (current library snapshot + current root)
    /// to the worker. The one constructor of worker-side disk stores, so a
    /// library edit or a root change can't leave the other half stale.
    fn refresh_disk_store(&self) {
        self.worker
            .set_disk_store(DiskStore::new(self.library.clone(), self.disk_root.clone()));
    }

    /// Compile `graph` against the current library and record the artifact's
    /// flatten map as the engine's current one — every send below installs its
    /// artifact on the worker, so the map here always mirrors the program the
    /// worker's next stats come from. A failure is reported to [`Self::status`]
    /// and returns `None` (nothing sent, worker untouched); a success clears
    /// the sticky error.
    fn compile(&mut self, graph: &Graph) -> Option<CompiledGraph> {
        match self.compiler.compile(graph, &self.library) {
            Ok(Compilation {
                compiled,
                flatten_map,
            }) => {
                self.flatten_map = flatten_map;
                self.status.error = None;
                Some(compiled)
            }
            Err(e) => {
                self.status.error(format!("compile failed: {e}"));
                None
            }
        }
    }

    /// Point the disk cache at `doc_path`'s project-local store
    /// (`<stem>.darkroom-cache/` beside the file), so disk-backed (`Disk`/`Both`)
    /// nodes reload across sessions. `None` (an unsaved document) is memory-only. Explicit-path cache
    /// nodes are unaffected — they always use their own path.
    pub(crate) fn set_document_cache(&mut self, doc_path: Option<&Path>) {
        self.disk_root = doc_path.map(prepare_document_cache_root);
        self.refresh_disk_store();
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

    /// Compile `graph` and evaluate only `node_id`'s upstream cone, delivering
    /// its outputs for the preview fetch ("run to this node").
    /// Returns whether it was sent — a compile failure is reported to
    /// [`Self::status`] and nothing reaches the worker. Results arrive via
    /// [`Self::drain_worker`].
    pub(crate) fn run_node(&mut self, graph: &Graph, node_id: NodeId) -> bool {
        let Some(compiled) = self.compile(graph) else {
            return false;
        };
        let address = compiled.node_address(node_id);
        self.worker.run_node(compiled, address);
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
}
