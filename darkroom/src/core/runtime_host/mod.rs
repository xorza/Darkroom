//! The runtime services behind the editor: the function library and the evaluation worker. `App`
//! owns one and adds the frontend orchestration on top.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{ErrorChain, SystemMemory};
use lens::MlModelPaths;
use scenarium::DiskStore;
use scenarium::{CompiledGraph, Compiler, DynamicValue, WorkerExited, WorkerReport};
use scenarium::{GraphSet, InstancePath, NodeAddr, ProgramId};

use crate::core::document::Document;
use crate::core::io::cache;
use crate::core::io::preferences::memory_headroom::MemoryHeadroom;
use crate::core::runtime_host::error::NoCompiledWork;
use crate::core::runtime_library::RuntimeLibrary;
use crate::core::wake::Wake;
use crate::core::worker_bridge::WorkerBridge;

pub(crate) mod error;

#[derive(Debug)]
pub(crate) struct RuntimeHost {
    pub(crate) library: RuntimeLibrary,
    worker: WorkerBridge,
    /// The active disk-store root (`None` = memory-only), remembered so a
    /// repeated [`set_document_cache`](Self::set_document_cache) can tell
    /// whether the root changed.
    disk_root: Option<DiskRoot>,
    /// Whether the last compiled program holds a disk-backed node, the one kind whose values reach
    /// the store.
    disk_backed: bool,
    /// The memory the process could allocate at launch, which an automatic headroom is a share
    /// of: read once, as the free memory a run leaves behind is what the headroom measures.
    allocatable_at_launch: u64,
    /// Long-lived so the lowering scratch is reused across compiles.
    compiler: Compiler,
}

/// A disk-store root, and whether it was created with its `.gitignore`: done once per root, and
/// only when a blob can land there. A new root starts unprepared, so no change can skip it.
#[derive(Debug)]
struct DiskRoot {
    path: PathBuf,
    prepared: bool,
}

/// What a [`RuntimeHost::set_document_cache`] call did to the disk-store root, and therefore
/// what the worker has to be told. The worker cannot decide this itself: a store is replaced for
/// several reasons, and only the host knows whether the document kept its identity across one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CacheRootChange {
    /// The same root: the store this would build is the one already installed. A plain re-save
    /// lands here, and rebuilding would cost a header read per resident disk-backed node.
    Unchanged,
    /// A different root, or none at all: a document opened, closed, or saved somewhere else. The
    /// worker takes the new store and writes nothing into it: each location keeps its own store
    /// and refills lazily, per [`cache::root`].
    Repointed,
    /// A document that had nowhere to persist gained a root: the first save of an unsaved
    /// document. **The one transition that owes a flush**: its values kept in RAM and on disk were computed
    /// memory-only, and a RAM hit never stores, so without one they would be served from RAM for
    /// the session and silently recompute on reopen.
    Gained,
}

impl CacheRootChange {
    fn of(prev: Option<&Path>, cur: Option<&Path>) -> Self {
        match (prev, cur) {
            _ if prev == cur => Self::Unchanged,
            (None, Some(_)) => Self::Gained,
            _ => Self::Repointed,
        }
    }
}

/// A compile and the program nodes of one authored node it holds work for, one per instance:
/// what an eviction or a flush of that node sends.
#[derive(Debug)]
struct NodeWork {
    compiled: Arc<CompiledGraph>,
    program_ids: Vec<ProgramId>,
}

impl RuntimeHost {
    /// Assemble the func library, its ML nodes defaulting to `ml_models`, and
    /// spin up the evaluation worker, which is woken through `wake` and keeps `headroom` free.
    pub(crate) fn new(wake: Wake, ml_models: &MlModelPaths, headroom: MemoryHeadroom) -> Self {
        let library = RuntimeLibrary::new(ml_models);
        let worker = WorkerBridge::new(wake);
        let host = Self {
            library,
            worker,
            disk_root: None,
            disk_backed: false,
            allocatable_at_launch: SystemMemory::available(),
            compiler: Compiler::default(),
        };
        // Memory-only until a document has a path; `set_document_cache` repoints the root.
        host.sync_worker_disk_store();
        host.set_memory_headroom(headroom);
        host
    }

    /// Keep `headroom` of memory free for everything else, past which the worker drops the kept
    /// node outputs used least recently.
    pub(crate) fn set_memory_headroom(&self, headroom: MemoryHeadroom) {
        let bytes = headroom.bytes(self.allocatable_at_launch);
        self.dispatch(|worker| worker.set_ram_headroom(bytes));
    }

    /// Re-seed the ML nodes' model-path defaults from `ml_models`.
    pub(crate) fn set_ml_model_paths(&mut self, ml_models: &MlModelPaths) {
        self.library.set_ml_model_paths(ml_models);
    }

    /// Point the worker's [`DiskStore`] at the current root.
    fn sync_worker_disk_store(&self) {
        let root = self.disk_root.as_ref().map(|root| root.path.clone());
        self.dispatch(|worker| worker.set_disk_store(DiskStore::new(root)));
    }

    /// Compile `graphs` against the current library. The document keeps its
    /// graphs ones the library can run, so a compile cannot fail.
    fn compile(&mut self, graphs: &GraphSet) -> Arc<CompiledGraph> {
        let compiled = self.compiler.compile(graphs, self.library.cur());
        self.disk_backed = compiled.persists_to_disk();
        self.prepare_root();
        Arc::new(compiled)
    }

    /// Drop the worker's installed program and the whole runtime cache behind it, as a document
    /// swap owes: nothing the outgoing document computed can serve the incoming one, and those
    /// values (decoded frames and GPU textures) are the editor's largest RAM tenant. Without it
    /// they sit resident until the *next* install reconciles them away, so a File ▸ New taken to
    /// free memory frees nothing.
    ///
    /// The disk store is untouched: [`Self::set_document_cache`] repoints it, and the blobs the
    /// old document wrote stay for when it is reopened.
    pub(crate) fn clear_program(&mut self) {
        self.disk_backed = false;
        self.dispatch(WorkerBridge::clear);
    }

    /// Create the disk root, with its `.gitignore`, before the first blob can land there: once
    /// per root, and only while the program holds a disk-backed node. A failure is logged and
    /// retried: the cache still works, as blob writes make the folder, but it would show in
    /// version control.
    fn prepare_root(&mut self) {
        if !self.disk_backed {
            return;
        }
        let Some(root) = self.disk_root.as_mut().filter(|root| !root.prepared) else {
            return;
        };
        match cache::prepare(&root.path) {
            Ok(()) => root.prepared = true,
            Err(error) => tracing::warn!(
                "the cache folder {} has no .gitignore: {}",
                root.path.display(),
                ErrorChain::new(&error)
            ),
        }
    }

    /// Point the disk cache at `document_path`'s project-local store (`<stem>.<extension>-cache/`
    /// beside the file), so disk-backed nodes reload across sessions. `None` (an
    /// unsaved document) is memory-only. Explicit-path cache nodes are unaffected: they always use
    /// their own path.
    ///
    /// What the worker is told is [`CacheRootChange`]'s to decide.
    pub(crate) fn set_document_cache(&mut self, document_path: Option<&Path>) {
        let cur = document_path.map(cache::root);
        let change = CacheRootChange::of(
            self.disk_root.as_ref().map(|root| root.path.as_path()),
            cur.as_deref(),
        );
        if change == CacheRootChange::Unchanged {
            return;
        }
        self.disk_root = cur.map(|path| DiskRoot {
            path,
            prepared: false,
        });
        // Before the store moves: a disk-backed program installed before the
        // change, a run in flight or an event loop, writes into the new root
        // at once, as the flush below does.
        self.prepare_root();
        // The attach must lead the flush: usually both reduce into one batch, where the worker's
        // apply order decides, but the worker can wake between the sends and split them, and then
        // the flush would land before the store it is for, writing into the root being left.
        self.sync_worker_disk_store();
        if change == CacheRootChange::Gained {
            self.dispatch(WorkerBridge::flush_all_caches);
        }
    }

    /// Compile `graphs` against the current library and send it to the worker
    /// for one evaluation. Results arrive via [`Self::drain_worker`].
    pub(crate) fn run_once(&mut self, graphs: &GraphSet) {
        let compiled = self.compile(graphs);
        self.dispatch(|worker| {
            worker.install(compiled)?;
            worker.run_sinks()
        });
    }

    /// Compile `document` and evaluate the upstream cone of each preview it runs on change,
    /// delivering their outputs: the previews alone are seeds, so no other sink runs. Nothing
    /// reaches the worker when the document runs none.
    pub(crate) fn run_previews(&mut self, document: &Document) {
        let compiled = self.compile(&document.graphs);
        let seeds: Vec<ProgramId> = compiled
            .program_ids_within(InstancePath::MAIN)
            .filter(|&program_id| document.runs_preview(program_id))
            .collect();
        if seeds.is_empty() {
            return;
        }
        self.dispatch(|worker| {
            worker.install(compiled)?;
            worker.run_nodes(seeds)
        });
    }

    /// Compile `graphs` and evaluate program node `program_id`, delivering its outputs
    /// for the preview fetch ("run to this node"). The node seeds the run
    /// explicitly, which overrides its `disabled` flag during planning.
    ///
    /// Nothing reaches the worker when the program holds no work for the
    /// node, which an edit in the same frame removed. Results arrive via
    /// [`Self::drain_worker`].
    pub(crate) fn run_node(
        &mut self,
        graphs: &GraphSet,
        program_id: ProgramId,
    ) -> Result<(), NoCompiledWork> {
        let compiled = self.compiled_for(graphs, program_id)?;
        self.dispatch(|worker| {
            worker.install(compiled)?;
            worker.run_nodes([program_id])
        });
        Ok(())
    }

    /// Compile `graphs`, refusing when the program holds no work for `program_id`.
    fn compiled_for(
        &mut self,
        graphs: &GraphSet,
        program_id: ProgramId,
    ) -> Result<Arc<CompiledGraph>, NoCompiledWork> {
        let compiled = self.compile(graphs);
        if compiled.contains(program_id) {
            Ok(compiled)
        } else {
            Err(NoCompiledWork)
        }
    }

    /// Compile the current graph and atomically install it with a runtime-cache eviction for
    /// every program node of the authored node `addr` — one per instance of its graph — and
    /// their compiled downstream cone.
    ///
    /// Answers with the nodes that eviction reaches, resolved against the very artifact it
    /// installs, so a caller drops exactly what the worker drops. The cone is a pure function of
    /// the artifact, and a successful eviction is fire-and-forget (the worker reports only
    /// failures), so no reply is read. Nothing reaches the worker when the program holds no work
    /// for `addr`, as for [`Self::run_node`].
    pub(crate) fn evict_cache(
        &mut self,
        graphs: &GraphSet,
        addr: NodeAddr,
    ) -> Result<Vec<ProgramId>, NoCompiledWork> {
        let NodeWork {
            compiled,
            program_ids,
        } = self.compiled_for_node(graphs, addr)?;
        let evicted = compiled.consumer_cone(program_ids.iter().copied());
        self.dispatch(|worker| worker.install_and_evict_cache(compiled, program_ids));
        Ok(evicted)
    }

    /// Compile the current graph and atomically install it with a request to persist the
    /// resident value of every program node of the authored node `addr`, which a node needs the
    /// moment its cache mode gains the disk bit, since a run that reuses the RAM value never
    /// publishes a blob. Every instance, as the cache mode is the authored node's.
    ///
    /// The install travels with it because the worker reads the cache mode off the installed
    /// program, still the pre-edit one here. Nothing reaches the worker when the program holds no
    /// work for `addr`.
    pub(crate) fn flush_cache(
        &mut self,
        graphs: &GraphSet,
        addr: NodeAddr,
    ) -> Result<(), NoCompiledWork> {
        let NodeWork {
            compiled,
            program_ids,
        } = self.compiled_for_node(graphs, addr)?;
        self.dispatch(|worker| worker.install_and_flush_cache(compiled, program_ids));
        Ok(())
    }

    /// Compile `graphs`, and name each program node of the authored node `addr` it holds work
    /// for, refusing when it holds none: for an instance, each program node inside it, as its
    /// cache mode reaches the values that leave it and a clear reaches what it holds.
    fn compiled_for_node(
        &mut self,
        graphs: &GraphSet,
        addr: NodeAddr,
    ) -> Result<NodeWork, NoCompiledWork> {
        let compiled = self.compile(graphs);
        let instance = graphs
            .graph(addr.graph)
            .find(addr.node_id)
            .is_some_and(|node| node.compound().is_some());
        let mut program_ids: Vec<ProgramId> = Vec::new();
        for path in graphs.instances(addr.graph) {
            if instance {
                program_ids.extend(compiled.program_ids_within(path.child(addr.node_id)));
            } else if compiled.contains(path.node(addr.node_id)) {
                program_ids.push(path.node(addr.node_id));
            }
        }
        if program_ids.is_empty() {
            Err(NoCompiledWork)
        } else {
            Ok(NodeWork {
                compiled,
                program_ids,
            })
        }
    }

    /// Request cancellation of the in-flight run: coarse, the running node finishes and nothing
    /// further is scheduled.
    pub(crate) fn cancel_run(&self) {
        self.worker.cancel_run();
    }

    /// Start the event loop on `graphs`. The worker's `Update` tears down any prior loop first.
    pub(crate) fn start_event_loop(&mut self, graphs: &GraphSet) {
        let compiled = self.compile(graphs);
        self.dispatch(|worker| {
            worker.install(compiled)?;
            worker.start_event_loop()
        });
    }

    /// Stop the event loop.
    pub(crate) fn stop_event_loop(&self) {
        self.dispatch(WorkerBridge::stop_event_loop);
    }

    /// Send a batch of worker commands.
    ///
    /// A send only fails once the worker task is gone, and the host owns that task for the whole
    /// session, stopping it once in [`WorkerBridge`]'s `Drop`. A failure therefore means the
    /// worker panicked: nothing can compute afterwards and no report will arrive, so it fails
    /// loudly rather than leave the editor alive and inert.
    fn dispatch(&self, commands: impl FnOnce(&WorkerBridge) -> Result<(), WorkerExited>) {
        if let Err(error) = commands(&self.worker) {
            let error = ErrorChain::new(&error);
            tracing::error!(%error, "worker task is gone; no command can be delivered");
            panic!("worker exited while the host was still running: {error}");
        }
    }

    /// Swap the worker results posted since the last frame into `reports`, which is empty; the
    /// returned count is of runs that ended in a result a later one superseded
    /// ([`WorkerBridge::drain`]).
    pub(crate) fn drain_worker(&self, reports: &mut VecDeque<WorkerReport>) -> u64 {
        self.worker.drain(reports)
    }

    /// Non-blocking drain of every preview value the worker's lambdas published since the last
    /// frame.
    ///
    /// Separate from [`Self::drain_worker`] because it does not travel the report stream: a
    /// preview node's lambda writes it directly. Ordering against the reports does not matter, as
    /// a value is only ever the *latest* for its node.
    pub(crate) fn drain_previews(&self, buffer: &mut HashMap<ProgramId, DynamicValue>) {
        self.library.previews.drain_into(buffer);
    }
}

#[cfg(test)]
mod tests;
