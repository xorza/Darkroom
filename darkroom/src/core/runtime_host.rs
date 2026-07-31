//! The runtime services behind the editor: the function library and the
//! evaluation worker. `App` owns one and adds the frontend orchestration on
//! top, so worker construction and the drain/run primitives live here rather
//! than in the shell.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use scenarium::DiskStore;
use scenarium::{
    CompiledGraph, Compiler, DynamicValue, ExecutionNodeId, WorkerExited, WorkerReport,
};
use scenarium::{Graph, NodeId};

use crate::core::io::cache::prepare_document_cache_root;
use crate::core::io::preferences::Preferences;
use crate::core::runtime_library::RuntimeLibrary;
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
    /// results, file ops) and renders it as the sticky error slot in the
    /// status bar.
    pub(crate) status: StatusLog,
}

impl RuntimeHost {
    /// Assemble the func library and spin up the evaluation worker, which is
    /// woken through `wake`.
    pub(crate) fn new(wake: Wake, preferences: &Preferences) -> Self {
        let model_paths = (&preferences.ml_models).into();
        let library = RuntimeLibrary::new(&model_paths);
        let worker = WorkerBridge::new(wake);
        let host = Self {
            library,
            worker,
            disk_root: None,
            compiler: Compiler::default(),
            status: StatusLog::default(),
        };
        // Install the store up front (memory-only until a document has a
        // path); `set_document_cache` repoints the root as documents open.
        host.sync_worker_disk_store();
        host
    }

    /// Re-seed the ML nodes' model-path defaults from `preferences`.
    pub(crate) fn configure_ml_model_defaults(&mut self, preferences: &Preferences) {
        let model_paths = (&preferences.ml_models).into();
        if self.library.update_ml_model_paths(&model_paths) {
            self.sync_worker_disk_store();
        }
    }

    /// Push a fresh [`DiskStore`] (current library snapshot + current root)
    /// to the worker. The one constructor of worker-side disk stores, so a
    /// library edit or a root change can't leave the other half stale.
    fn sync_worker_disk_store(&self) {
        self.worker.set_disk_store(DiskStore::new(
            &self.library.published.load(),
            self.disk_root.clone(),
        ));
    }

    /// Compile `graph` against the current library. A failure is reported to
    /// [`Self::status`] and returns `None` (nothing sent, worker untouched); a
    /// success clears the sticky error.
    fn compile(&mut self, graph: &Graph) -> Option<Arc<CompiledGraph>> {
        match self.compiler.compile(graph, &self.library.published.load()) {
            Ok(compiled) => {
                self.status.error = None;
                Some(Arc::new(compiled))
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
    /// for one evaluation. `false` means the compile failed — it is reported
    /// to [`Self::status`] synchronously and nothing reaches the worker.
    /// Results arrive via [`Self::drain_worker`].
    pub(crate) fn run_once(&mut self, graph: &Graph) -> bool {
        let Some(compiled) = self.compile(graph) else {
            return false;
        };
        self.dispatch(|worker| {
            worker.install(compiled)?;
            worker.run_sinks()
        });
        true
    }

    /// Compile `graph` and evaluate authored `node_id`, delivering its outputs
    /// for the preview fetch ("run to this node"). The node seeds the run
    /// explicitly, which overrides its `disabled` flag during planning.
    ///
    /// `false` means nothing reached the worker — either the compile failed
    /// (reported to [`Self::status`]) or the node has no execution footprint at
    /// all, i.e. the program dropped it. Results arrive via
    /// [`Self::drain_worker`].
    pub(crate) fn run_node(&mut self, graph: &Graph, node_id: NodeId) -> bool {
        let Some(compiled) = self.compile(graph) else {
            return false;
        };
        let Some(target) = compiled.run_target(node_id) else {
            self.status
                .error("nothing to run: this node has no compiled work".to_owned());
            return false;
        };
        self.dispatch(|worker| {
            worker.install(compiled)?;
            worker.run_nodes(vec![target])
        });
        true
    }

    /// Compile the current graph and atomically install it with a runtime-cache
    /// eviction for `node_id` and its flattened downstream cone.
    pub(crate) fn evict_cache(&mut self, graph: &Graph, node_id: NodeId) -> bool {
        let Some(compiled) = self.compile(graph) else {
            return false;
        };
        self.dispatch(|worker| worker.install_and_evict_cache(compiled, node_id));
        true
    }

    /// Request cancellation of the in-flight run (coarse — the running node
    /// finishes, nothing further is scheduled).
    pub(crate) fn cancel_run(&self) {
        self.worker.cancel_run();
    }

    /// Start the event loop on `graph` (compiles + loads it, then fires
    /// events). The worker's `Update` tears down any prior loop first.
    /// `false` means the compile failed — it is reported to [`Self::status`]
    /// and the loop's running state is untouched.
    pub(crate) fn start_event_loop(&mut self, graph: &Graph) -> bool {
        let Some(compiled) = self.compile(graph) else {
            return false;
        };
        self.dispatch(|worker| {
            worker.install(compiled)?;
            worker.start_event_loop()
        });
        true
    }

    /// Stop the event loop. Best-effort: a worker that has already exited
    /// has no loop left to stop, which is the outcome the caller wanted.
    pub(crate) fn stop_event_loop(&self) {
        self.worker.stop_event_loop();
    }

    /// Send a batch of worker commands.
    ///
    /// A send only fails once the worker task is gone, and the host owns
    /// that task for the whole session — it is stopped exactly once, in
    /// [`WorkerBridge`]'s `Drop`. Reaching here therefore means the worker
    /// panicked: a broken invariant, not a condition to report and carry
    /// on from. Nothing can compute afterwards and no report will ever
    /// arrive, so the honest response is to fail loudly rather than leave
    /// the editor alive and inert.
    fn dispatch(&self, commands: impl FnOnce(&WorkerBridge) -> Result<(), WorkerExited>) {
        if let Err(error) = commands(&self.worker) {
            tracing::error!(%error, "worker task is gone; no command can be delivered");
            panic!("worker exited while the host was still running: {error}");
        }
    }

    /// Non-blocking drain of worker results posted since the last frame.
    ///
    /// Owned rather than an iterator: the caller writes back through `self`
    /// while handling a report, so a borrowing iterator would have to be
    /// collected at the call site anyway.
    pub(crate) fn drain_worker(&self) -> Vec<WorkerReport> {
        self.worker.drain().collect()
    }

    /// Non-blocking drain of every preview value the worker's lambdas published
    /// since the last frame. Empty on an idle frame.
    ///
    /// Separate from [`Self::drain_worker`] because it does not travel the
    /// report stream: a preview node's lambda writes it directly. Ordering
    /// against the reports does not matter — a value is only ever the *latest*
    /// for its node, never a step in a sequence.
    pub(crate) fn drain_previews(&self) -> Vec<(ExecutionNodeId, DynamicValue)> {
        self.library.previews.drain()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use common::internals::test_output_path;
    use scenarium::{Binding, Graph, InputPort, NodeId, StaticValue};

    use crate::core::io::cache::document_cache_root;
    use crate::core::io::preferences::{MlModelPreferences, Preferences};
    use crate::core::runtime_host::RuntimeHost;

    /// The default value seeded into `func`'s model-path input (index 1),
    /// read back through the published library.
    fn ml_model_default(host: &RuntimeHost, func: &str) -> Option<StaticValue> {
        host.library.published.load().by_name(func).unwrap().inputs[1]
            .default_value
            .clone()
    }

    #[test]
    fn stale_wiring_survives_compilation_and_still_runs() {
        let mut host = RuntimeHost::new(Arc::new(|| {}), &Preferences::default());

        // Library drift: a wire into an output the func doesn't declare.
        // Nothing prunes it — the authored wiring stays (it revives if the
        // library gets the port back) and compilation tolerates it as an
        // unbound input.
        let library = host.library.published.load();
        let func = library.by_name("ML Denoise").expect("built-in present");
        let mut graph = Graph::default();
        let producer = graph.add_func_node(func);
        let consumer = graph.add_func_node(func);
        let dangling = InputPort::new(consumer, 0);
        graph.set_input_binding(dangling, Binding::bind(producer, 99));
        drop(library);

        assert!(
            host.run_once(&graph),
            "the drifted graph compiles and is queued"
        );
        assert!(
            host.evict_cache(&graph, NodeId::unique()),
            "the drifted graph compiles and queues an eviction"
        );
        assert_eq!(host.status.error, None, "no compile failure was reported");
        assert_eq!(
            graph.bindings.get(&dangling),
            Some(&Binding::bind(producer, 99)),
            "the dangling wire is preserved, not pruned"
        );
    }

    #[test]
    fn ml_defaults_follow_preferences_and_the_cache_root_follows_the_document() {
        let denoise_path = "/models/host-denoise.onnx";
        let star_removal_path = "/models/host-stars.onnx";
        let mut preferences = Preferences {
            ml_models: MlModelPreferences {
                denoise: denoise_path.into(),
                star_removal: star_removal_path.into(),
            },
            ..Preferences::default()
        };
        let mut host = RuntimeHost::new(Arc::new(|| {}), &preferences);

        // Startup seeds the ML nodes' path inputs from the preferences.
        assert_eq!(
            ml_model_default(&host, "ML Denoise"),
            Some(StaticValue::FsPath(denoise_path.to_owned()))
        );
        assert_eq!(
            ml_model_default(&host, "ML Star Removal"),
            Some(StaticValue::FsPath(star_removal_path.to_owned()))
        );

        // A later preferences edit republishes the library with the new paths.
        let updated_denoise_path = "/models/updated-denoise.onnx";
        let updated_star_removal_path = "/models/updated-stars.onnx";
        preferences.ml_models.denoise = updated_denoise_path.into();
        preferences.ml_models.star_removal = updated_star_removal_path.into();
        host.configure_ml_model_defaults(&preferences);
        assert_eq!(
            ml_model_default(&host, "ML Denoise"),
            Some(StaticValue::FsPath(updated_denoise_path.to_owned()))
        );
        assert_eq!(
            ml_model_default(&host, "ML Star Removal"),
            Some(StaticValue::FsPath(updated_star_removal_path.to_owned()))
        );

        // The disk cache is memory-only until a document has a path, then
        // repoints as documents open and again when the path goes away.
        let first_path = test_output_path("darkroom_runtime_host/first.darkroom");
        let second_path = test_output_path("darkroom_runtime_host/second.darkroom");
        assert_eq!(host.disk_root, None);
        host.set_document_cache(Some(&first_path));
        assert_eq!(host.disk_root, Some(document_cache_root(&first_path)));
        host.set_document_cache(Some(&second_path));
        assert_eq!(host.disk_root, Some(document_cache_root(&second_path)));
        host.set_document_cache(None);
        assert_eq!(host.disk_root, None);
    }
}
