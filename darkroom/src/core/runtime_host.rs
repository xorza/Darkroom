//! The runtime services shared by every frontend: the function library,
//! the evaluation worker, and the scripting-over-TCP host. `App` (GUI) and
//! `TerminalSession` (tui/headless) share one through a `Workspace` and add
//! frontend orchestration on top, so worker/script construction and the
//! drain/run primitives live here once instead of in both shells.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use scenarium::DiskStore;
use scenarium::FlattenMap;
use scenarium::Library;
use scenarium::{Compilation, CompiledGraph, Compiler};
use scenarium::{Graph, NodeId};

use crate::core::io::cache::prepare_document_cache_root;
use crate::core::library::RuntimeLibrary;
use crate::core::script::{ScriptConfig, ScriptHost, ScriptMessage};
use crate::core::status::StatusLog;
use crate::core::wake::Wake;
use crate::core::worker::{WorkerBridge, WorkerEvent};

#[derive(Debug)]
pub(crate) struct RuntimeHost {
    /// Darkroom-owned runtime library state and its published snapshots.
    pub(crate) library: RuntimeLibrary,
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

impl RuntimeHost {
    /// Assemble the func lib (builtins + the on-disk graph library), spin
    /// up the evaluation worker, and start the script host (a no-op `None`
    /// unless `script_cfg` enabled a listener). The worker + script host are
    /// both woken through `wake`.
    pub(crate) fn new(
        script_cfg: &ScriptConfig,
        wake: Wake,
        model_paths: &lens::MlModelPaths,
    ) -> Self {
        let loaded = RuntimeLibrary::load(model_paths);
        let worker = WorkerBridge::new(wake.clone());
        let script = ScriptHost::start(script_cfg, loaded.library.published.clone(), wake);
        let mut host = Self {
            library: loaded.library,
            worker,
            disk_root: None,
            compiler: Compiler::default(),
            flatten_map: Arc::default(),
            status: StatusLog::default(),
            script,
        };
        // Install the store up front (memory-only until a document has a
        // path); `set_document_cache` repoints the root as documents open.
        host.refresh_disk_store();
        if let Some(err) = loaded.error {
            host.status.error(format!("library load failed: {err:#}"));
        }
        host
    }

    pub(crate) fn configure_ml_model_defaults(&mut self, paths: &lens::MlModelPaths) {
        if self.library.update_ml_model_paths(paths) {
            self.refresh_disk_store();
        }
    }

    /// Applies a graph-library edit, publishes the new runtime snapshot to
    /// scripting, persists shared graphs, and refreshes the worker's
    /// [`DiskStore`].
    ///
    /// Owns the persist outcome in [`Self::status`]: a saved change clears
    /// the sticky error, a failed save reports — callers must not overwrite
    /// it with their own success signal.
    pub(crate) fn edit_library(&mut self, edit: impl FnOnce(&mut Library) -> bool) -> bool {
        let outcome = self.library.edit_shared_graphs(edit);
        if outcome.changed {
            match outcome.persist_error {
                None => self.status.error = None,
                Some(err) => self.status.error(format!("library save failed: {err:#}")),
            }
            self.refresh_disk_store();
        }
        outcome.changed
    }

    /// Push a fresh [`DiskStore`] (current library snapshot + current root)
    /// to the worker. The one constructor of worker-side disk stores, so a
    /// library edit or a root change can't leave the other half stale.
    fn refresh_disk_store(&self) {
        self.worker.set_disk_store(DiskStore::new(
            self.library.current.clone(),
            self.disk_root.clone(),
        ));
    }

    /// Compile `graph` against the current library and record the artifact's
    /// flatten map as the host's current one — every send below installs its
    /// artifact on the worker, so the map here always mirrors the program the
    /// worker's next stats come from. A failure is reported to [`Self::status`]
    /// and returns `None` (nothing sent, worker untouched); a success clears
    /// the sticky error.
    fn compile(&mut self, graph: &Graph) -> Option<CompiledGraph> {
        match self.compiler.compile(graph, &self.library.current) {
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
    /// its outputs for the preview fetch ("run to this node"). The explicit
    /// node seed overrides a compiled disabled target during planning.
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

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;

    use crate::core::runtime_host::RuntimeHost;

    pub(crate) fn disk_root(host: &RuntimeHost) -> Option<PathBuf> {
        host.disk_root.clone()
    }
}
