//! The runtime services behind the editor: the function library and the
//! evaluation worker. `App` owns one and adds the frontend orchestration on
//! top, so worker construction and the drain/run primitives live here rather
//! than in the shell.

use std::collections::HashMap;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use scenarium::DiskStore;
use scenarium::{CompiledGraph, Compiler, DynamicValue, WorkerExited, WorkerReport};
use scenarium::{Graph, NodeId};

use crate::core::io::cache;
use crate::core::io::preferences::Preferences;
use crate::core::runtime_library::RuntimeLibrary;
use crate::core::status::{StatusFamily, StatusLog};
use crate::core::wake::Wake;
use crate::core::worker::WorkerBridge;

#[derive(Debug)]
pub(crate) struct RuntimeHost {
    pub(crate) library: RuntimeLibrary,
    worker: WorkerBridge,
    /// The active disk-store root (`None` = memory-only), remembered so a
    /// repeated [`set_document_cache`](Self::set_document_cache) can tell
    /// whether the root changed.
    disk_root: Option<PathBuf>,
    /// Whether `disk_root` was created, with its `.gitignore`. Done once per
    /// root, and only when a blob can land there.
    root_prepared: bool,
    /// Whether the last compiled program holds a disk-backed (`Disk`/`Both`)
    /// node — the one kind whose values reach the store.
    disk_backed: bool,
    /// Long-lived so the lowering scratch is reused across compiles instead of
    /// reallocated per run.
    compiler: Compiler,
}

/// What a [`RuntimeHost::set_document_cache`] call did to the disk-store root,
/// and therefore what the worker has to be told.
///
/// Named rather than inlined because the interesting part is which transitions
/// *don't* act. The worker cannot decide this for itself: a store is replaced
/// for several unrelated reasons, and only the host knows whether the document
/// kept its identity across one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CacheRootChange {
    /// The same root: the store this would build is the one already installed.
    /// A plain re-save lands here, and rebuilding would cost a header read per
    /// resident disk-backed node for nothing.
    Unchanged,
    /// A different root, or none at all — a document opened, closed, or saved
    /// somewhere else. The worker takes the new store and writes nothing into
    /// it: each location keeps its own store and refills lazily, per
    /// [`document_cache_root`](cache::document_cache_root).
    Repointed,
    /// A document that had nowhere to persist gained a root — the first save of
    /// an unsaved document. **The one transition that owes a flush**: its
    /// `Both`-mode values were computed memory-only, and a RAM hit never
    /// stores, so without one they would be served from RAM for the rest of the
    /// session and silently recompute on reopen.
    Gained,
}

impl CacheRootChange {
    fn of(previous: Option<&Path>, current: Option<&Path>) -> Self {
        match (previous, current) {
            _ if previous == current => Self::Unchanged,
            (None, Some(_)) => Self::Gained,
            _ => Self::Repointed,
        }
    }
}

impl RuntimeHost {
    /// Assemble the func library and spin up the evaluation worker, which is
    /// woken through `wake`.
    pub(crate) fn new(wake: Wake, preferences: &Preferences) -> Self {
        let library = RuntimeLibrary::new(&preferences.ml_models);
        let worker = WorkerBridge::new(wake);
        let host = Self {
            library,
            worker,
            disk_root: None,
            root_prepared: false,
            disk_backed: false,
            compiler: Compiler::default(),
        };
        // Install the store up front (memory-only until a document has a
        // path); `set_document_cache` repoints the root as documents open.
        host.sync_worker_disk_store();
        host
    }

    /// Re-seed the ML nodes' model-path defaults from `preferences`.
    pub(crate) fn configure_ml_model_defaults(&mut self, preferences: &Preferences) {
        self.library.update_ml_model_paths(&preferences.ml_models);
    }

    /// Point the worker's [`DiskStore`] at the current root.
    fn sync_worker_disk_store(&self) {
        self.dispatch(|worker| worker.set_disk_store(DiskStore::new(self.disk_root.clone())));
    }

    /// Compile `graph` against the current library. A failure is reported to
    /// `status` and returns `None` (nothing sent, worker untouched); a success
    /// clears the sticky error.
    ///
    /// The log is borrowed rather than owned: it is the app's user-facing
    /// outcome slot, shared with file and preferences reporting, and this host
    /// is otherwise headless.
    fn compile(&mut self, graph: &Graph, status: &mut StatusLog) -> Option<Arc<CompiledGraph>> {
        match self.compiler.compile(graph, self.library.current()) {
            Ok(compiled) => {
                status.succeeded(StatusFamily::Run);
                self.disk_backed = graph.iter().any(|node| node.cache.persists_to_disk());
                self.prepare_root();
                Some(Arc::new(compiled))
            }
            Err(e) => {
                status.error(StatusFamily::Run, format!("compile failed: {e}"));
                None
            }
        }
    }

    /// Drop the worker's installed program and the whole runtime cache behind
    /// it — what a document swap owes, since nothing the outgoing document
    /// computed can serve the incoming one, and those values are the editor's
    /// largest RAM tenant by far (decoded frames and GPU textures).
    ///
    /// Without it the old document's results sit resident until the *next*
    /// install reconciles them away, which is whenever the user next runs
    /// something — so a File ▸ New taken to free memory frees nothing.
    ///
    /// The disk store is untouched: it is repointed separately by
    /// [`Self::set_document_cache`], and the blobs the old document wrote stay
    /// where they are for when it is reopened.
    pub(crate) fn clear_program(&mut self) {
        self.disk_backed = false;
        self.dispatch(WorkerBridge::clear);
    }

    /// Create the disk root, with its `.gitignore`, before the first blob can
    /// land there: once per root, and only while the program holds a
    /// disk-backed node.
    fn prepare_root(&mut self) {
        if self.root_prepared || !self.disk_backed {
            return;
        }
        if let Some(root) = &self.disk_root {
            cache::prepare_cache_root(root);
            self.root_prepared = true;
        }
    }

    /// Point the disk cache at `doc_path`'s project-local store (`<stem>.darkroom-cache/` beside
    /// the file), so disk-backed (`Disk`/`Both`) nodes reload across sessions. `None` (an unsaved
    /// document) is memory-only. Explicit-path cache nodes are unaffected — they always use their
    /// own path.
    ///
    /// What the worker is told is [`CacheRootChange`]'s to decide — see there
    /// for why only one of the transitions owes a flush.
    pub(crate) fn set_document_cache(&mut self, doc_path: Option<&Path>) {
        let previous = mem::replace(
            &mut self.disk_root,
            doc_path.map(cache::document_cache_root),
        );
        let change = CacheRootChange::of(previous.as_deref(), self.disk_root.as_deref());
        if change != CacheRootChange::Unchanged {
            self.root_prepared = false;
        }
        match change {
            CacheRootChange::Unchanged => {}
            CacheRootChange::Repointed => self.sync_worker_disk_store(),
            CacheRootChange::Gained => {
                // The flush writes the resident disk-backed values, so the root
                // they land in is prepared first.
                self.prepare_root();
                // The attach leads, and the two are not interchangeable. They
                // usually reduce into one batch, where the worker's apply order
                // decides and this order is moot — but the worker can wake
                // between the sends and split them, and then the sweep would
                // land in the batch *before* the store it is for, writing into
                // the root being left behind.
                self.sync_worker_disk_store();
                self.dispatch(WorkerBridge::flush_all_caches);
            }
        }
    }

    /// Compile `graph` against the current library and send it to the worker
    /// for one evaluation. `false` means the compile failed — it is reported
    /// to the caller's [`StatusLog`] synchronously and nothing reaches the
    /// worker.
    /// Results arrive via [`Self::drain_worker`].
    pub(crate) fn run_once(&mut self, graph: &Graph, status: &mut StatusLog) -> bool {
        let Some(compiled) = self.compile(graph, status) else {
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
    /// (reported to the caller's [`StatusLog`]) or the node has no execution
    /// footprint at
    /// all, i.e. the program dropped it. Results arrive via
    /// [`Self::drain_worker`].
    pub(crate) fn run_node(
        &mut self,
        graph: &Graph,
        node_id: NodeId,
        status: &mut StatusLog,
    ) -> bool {
        let Some(compiled) = self.compile(graph, status) else {
            return false;
        };
        if !compiled.contains(node_id) {
            status.error(
                StatusFamily::Run,
                "nothing to run: this node has no compiled work".to_owned(),
            );
            return false;
        }
        self.dispatch(|worker| {
            worker.install(compiled)?;
            worker.run_nodes(vec![node_id])
        });
        true
    }

    /// Compile the current graph and atomically install it with a runtime-cache
    /// eviction for `node_id` and its compiled downstream cone.
    ///
    /// Answers with the nodes that eviction reaches, resolved against the very
    /// artifact it installs — so a caller projecting the outcome drops exactly
    /// what the worker drops. Empty when nothing was dispatched: the compile
    /// failed (reported to `status`), or the program holds no work for
    /// `node_id`.
    ///
    /// Asking here rather than reading a reply: a successful eviction is
    /// fire-and-forget (the worker reports only failures), and the cone is a
    /// pure function of the artifact, so the answer is already in hand.
    pub(crate) fn evict_cache(
        &mut self,
        graph: &Graph,
        node_id: NodeId,
        status: &mut StatusLog,
    ) -> Vec<NodeId> {
        let Some(compiled) = self.compile(graph, status) else {
            return Vec::new();
        };
        let evicted = compiled.consumer_cone([node_id]);
        self.dispatch(|worker| worker.install_and_evict_cache(compiled, node_id));
        evicted
    }

    /// Compile the current graph and atomically install it with a request to
    /// persist `node_id`'s resident value — what a node needs the moment its
    /// cache mode gains the disk bit, since a run that reuses the RAM value
    /// never publishes a blob of its own.
    ///
    /// The install has to travel with it: the worker reads the node's cache mode
    /// off the installed program, which is still the pre-edit one here.
    pub(crate) fn flush_cache(
        &mut self,
        graph: &Graph,
        node_id: NodeId,
        status: &mut StatusLog,
    ) -> bool {
        let Some(compiled) = self.compile(graph, status) else {
            return false;
        };
        self.dispatch(|worker| worker.install_and_flush_cache(compiled, node_id));
        true
    }

    /// Request cancellation of the in-flight run (coarse — the running node
    /// finishes, nothing further is scheduled).
    pub(crate) fn cancel_run(&self) {
        self.worker.cancel_run();
    }

    /// Start the event loop on `graph` (compiles + loads it, then fires
    /// events). The worker's `Update` tears down any prior loop first.
    /// `false` means the compile failed — it is reported to the caller's
    /// [`StatusLog`] and the loop's running state is untouched.
    pub(crate) fn start_event_loop(&mut self, graph: &Graph, status: &mut StatusLog) -> bool {
        let Some(compiled) = self.compile(graph, status) else {
            return false;
        };
        self.dispatch(|worker| {
            worker.install(compiled)?;
            worker.start_event_loop()
        });
        true
    }

    /// Stop the event loop.
    pub(crate) fn stop_event_loop(&self) {
        self.dispatch(WorkerBridge::stop_event_loop);
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
    pub(crate) fn drain_worker(&self) -> impl Iterator<Item = WorkerReport> + '_ {
        self.worker.drain()
    }

    /// Non-blocking drain of every preview value the worker's lambdas published
    /// since the last frame. Empty on an idle frame.
    ///
    /// Separate from [`Self::drain_worker`] because it does not travel the
    /// report stream: a preview node's lambda writes it directly. Ordering
    /// against the reports does not matter — a value is only ever the *latest*
    /// for its node, never a step in a sequence.
    pub(crate) fn drain_previews(&self, buffer: &mut HashMap<NodeId, DynamicValue>) {
        self.library.previews.drain_into(buffer);
    }
}

#[cfg(test)]
mod tests;
