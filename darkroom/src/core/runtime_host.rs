//! The runtime services shared by every frontend: the function library,
//! the evaluation worker, and the scripting-over-TCP host. `App` (GUI) and
//! `TerminalSession` (tui/headless) share one through a `Workspace` and add
//! frontend orchestration on top, so worker/script construction and the
//! drain/run primitives live here once instead of in both shells.

use scenarium::DiskStore;
use scenarium::{CompiledGraph, Compiler, WorkerReport};
use scenarium::{Graph, NodeId};
use std::path::{Path, PathBuf};

use crate::core::document::{Document, GraphRef};
use crate::core::io::cache::prepare_document_cache_root;
use crate::core::io::preferences::Preferences;
use crate::core::runtime_library::{RuntimeLibrary, RuntimeLibraryChange};
use crate::core::script::{ScriptConfig, ScriptHost, ScriptMessage};
use crate::core::status::StatusLog;
use crate::core::wake::Wake;
use crate::core::worker::WorkerBridge;

#[derive(Debug)]
pub(crate) struct RuntimeHost {
    pub(crate) library: RuntimeLibrary,
    worker: WorkerBridge,
    /// The active disk-store root (`None` = memory-only),
    /// remembered so graph-library operations can re-push the worker's
    /// [`DiskStore`] — which carries a library snapshot — without the caller
    /// re-supplying the document path.
    disk_root: Option<PathBuf>,
    /// Long-lived so the flatten scratch is reused across compiles instead of
    /// reallocated per run.
    compiler: Compiler,
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
        preferences: &Preferences,
        mut status: StatusLog,
    ) -> Self {
        let model_paths = (&preferences.ml_models).into();
        let library = match RuntimeLibrary::load(&model_paths) {
            Ok(library) => library,
            Err(error) => {
                status.error(format!("graph library load failed: {error}"));
                RuntimeLibrary::new(&model_paths)
            }
        };
        let worker = WorkerBridge::new(wake.clone());
        let script = ScriptHost::start(script_cfg, library.published.clone(), wake);
        let host = Self {
            library,
            worker,
            disk_root: None,
            compiler: Compiler::default(),
            status,
            script,
        };
        // Install the store up front (memory-only until a document has a
        // path); `set_document_cache` repoints the root as documents open.
        host.sync_worker_disk_store();
        host
    }

    pub(crate) fn configure_ml_model_defaults(&mut self, preferences: &Preferences) {
        let model_paths = (&preferences.ml_models).into();
        if self.library.update_ml_model_paths(&model_paths) {
            self.sync_worker_disk_store();
        }
    }

    pub(crate) fn import_template(&mut self, graph: Graph) -> bool {
        let change = self.library.import_template(graph);
        self.apply_library_change(change)
    }

    pub(crate) fn publish_graph_to_library(
        &mut self,
        document: &mut Document,
        target: GraphRef,
        node_id: NodeId,
    ) -> bool {
        let change = self.library.publish_graph(document, target, node_id);
        self.apply_library_change(change)
    }

    fn apply_library_change(&mut self, change: RuntimeLibraryChange) -> bool {
        let changed = change.changed;
        if changed {
            match change.persist_error {
                None => self.status.error = None,
                Some(error) => self
                    .status
                    .error(format!("graph library save failed: {error:#}")),
            }
            self.sync_worker_disk_store();
        }
        changed
    }

    /// Push a fresh [`DiskStore`] (current library snapshot + current root)
    /// to the worker. The one constructor of worker-side disk stores, so a
    /// library edit or a root change can't leave the other half stale.
    fn sync_worker_disk_store(&self) {
        self.worker.set_disk_store(DiskStore::new(
            self.library.published.load(),
            self.disk_root.clone(),
        ));
    }

    /// Compile `graph` against the current library. A failure is reported to
    /// [`Self::status`] and returns `None` (nothing sent, worker untouched); a
    /// success clears the sticky error.
    fn compile(&mut self, graph: &Graph) -> Option<CompiledGraph> {
        match self.compiler.compile(graph, &self.library.published.load()) {
            Ok(compiled) => {
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
        self.sync_worker_disk_store();
    }

    /// Compile `graph` against the current library and send it to the worker
    /// for one evaluation. Returns whether it was sent — a compile failure is
    /// reported to [`Self::status`] synchronously and nothing reaches the
    /// worker. Results arrive via [`Self::drain_worker`].
    pub(crate) fn run_once(&mut self, graph: &Graph) -> bool {
        let Some(compiled) = self.compile(graph) else {
            return false;
        };
        self.worker.install(compiled);
        self.worker.run_sinks();
        true
    }

    /// Compile `graph` and evaluate every occurrence of authored `node_id`,
    /// delivering their outputs for the preview fetch ("run to this node").
    /// The explicit node seed overrides disabled occurrences during planning.
    /// Returns whether it was sent — a compile failure is reported to
    /// [`Self::status`] and nothing reaches the worker. Results arrive via
    /// [`Self::drain_worker`].
    pub(crate) fn run_node(&mut self, graph: &Graph, node_id: NodeId) -> bool {
        let Some(compiled) = self.compile(graph) else {
            return false;
        };
        self.worker.install(compiled);
        self.worker.run_node(node_id);
        true
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
        self.worker.install(compiled);
        self.worker.start_event_loop();
        true
    }

    /// Stop the event loop.
    pub(crate) fn stop_event_loop(&self) {
        self.worker.stop_event_loop();
    }

    /// Non-blocking drain of worker results posted since the last frame.
    pub(crate) fn drain_worker(&self) -> impl Iterator<Item = WorkerReport> + '_ {
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
