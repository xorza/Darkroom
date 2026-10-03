//! The bridge between the editor's `RuntimeHost` and the background
//! graph-evaluation `Worker` (`scenarium::worker`). The worker runs on its
//! own tokio runtime; this type owns that runtime, the worker handle, and a
//! sync channel the worker's callback posts results onto. The host loop
//! ([`App::update`](crate::gui::app::App)) drains the channel each frame and is
//! woken from off-thread via the [`Wake`] callback.
//!
//! Outbound commands retain FIFO order, with program installation separate
//! from execution; inbound is a plain `std::sync::mpsc` because the consumer
//! is the synchronous frame loop on the main thread.

use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use scenarium::CompiledGraph;
use scenarium::DiskStore;
use scenarium::NodeId;
use scenarium::{Worker, WorkerExited, WorkerMessage, WorkerReport};

use crate::core::background_runtime::BackgroundRuntime;
use crate::core::wake::Wake;

/// Every command here answers whether the worker took it. A send only fails
/// once the worker task is gone, and then *no report will ever arrive* — so a
/// caller that assumed success would leave the UI waiting on a run that can
/// never report back.
pub(crate) struct WorkerBridge {
    worker: Worker,
    rx: Receiver<WorkerReport>,
    runtime: BackgroundRuntime,
}

impl Debug for WorkerBridge {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerBridge").finish_non_exhaustive()
    }
}

impl WorkerBridge {
    /// Spin up the worker on a fresh multi-thread runtime. Starts memory-only; the
    /// host installs the disk-backed output cache (codec registry + store
    /// root) via [`Self::set_disk_store`]. The callback runs on a worker thread:
    /// it forwards the result over `tx` and asks the host to paint, so the next
    /// frame drains it.
    pub(crate) fn new(wake: Wake) -> Self {
        let runtime = BackgroundRuntime::new().expect("build worker runtime");
        let (tx, rx) = channel::<WorkerReport>();
        // `Worker`'s `tokio::spawn` needs an ambient runtime.
        let worker = runtime.enter(|| Worker::new(move |report| Self::deliver(&tx, &wake, report)));
        Self {
            worker,
            rx,
            runtime,
        }
    }

    #[expect(
        clippy::let_underscore_must_use,
        reason = "a report for a closed session has no reader, and dropping it is correct"
    )]
    fn deliver(tx: &Sender<WorkerReport>, wake: &Wake, report: WorkerReport) {
        let _ = tx.send(report);
        (wake)();
    }

    /// Install a compiled program. The worker acknowledges it with
    /// `WorkerReport::Installed` before processing reports from later commands.
    pub(crate) fn install(&self, compiled: Arc<CompiledGraph>) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::Update { compiled })
    }

    /// Drop the installed program and everything its cache holds. The worker
    /// acknowledges it with `WorkerReport::Cleared`.
    pub(crate) fn clear(&self) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::Clear)
    }

    /// Install the current program and evict an authored node's cache cone as
    /// one worker commit. Stopping the event loop keeps it from immediately
    /// repopulating the entries being removed.
    pub(crate) fn install_and_evict_cache(
        &self,
        compiled: Arc<CompiledGraph>,
        node_id: NodeId,
    ) -> Result<(), WorkerExited> {
        self.worker.send_many([
            WorkerMessage::Update { compiled },
            WorkerMessage::EvictCache {
                nodes: vec![node_id],
            },
            WorkerMessage::StopEventLoop,
        ])
    }

    /// Install the current program and persist an authored node's resident
    /// disk-backed value as one worker commit. The install has to lead: the
    /// engine reads the node's cache mode off the *installed* program, so a
    /// flush sent against the previous one would find the node not yet
    /// disk-backed and write nothing.
    ///
    /// No `StopEventLoop`, unlike the eviction beside it — a flush publishes
    /// what is already in RAM and leaves nothing for a running loop to
    /// repopulate.
    pub(crate) fn install_and_flush_cache(
        &self,
        compiled: Arc<CompiledGraph>,
        node_id: NodeId,
    ) -> Result<(), WorkerExited> {
        self.worker.send_many([
            WorkerMessage::Update { compiled },
            WorkerMessage::FlushCache {
                nodes: vec![node_id],
            },
        ])
    }

    /// Execute every sink in the installed program.
    pub(crate) fn run_sinks(&self) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::RunSinks)
    }

    /// Execute these exact nodes in the installed program and deliver their
    /// outputs. Plural because a run is seeded with a set — a "run to this
    /// node" contributes one.
    pub(crate) fn run_nodes(&self, node_ids: Vec<NodeId>) -> Result<(), WorkerExited> {
        self.worker
            .send(WorkerMessage::RunNodes { nodes: node_ids })
    }

    /// Point the engine's output cache at another store root — e.g. the
    /// active document's. Takes effect before the next run's compile.
    /// Attaching writes nothing; see [`Self::flush_all_caches`].
    pub(crate) fn set_disk_store(&self, cache: DiskStore) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::SetDiskStore(cache))
    }

    /// Write every installed node's resident disk-backed value into the
    /// attached store — what the store is owed by values computed while there
    /// was nowhere to put them. Ordered after any `SetDiskStore` in the same
    /// batch by the worker's apply order, so the two travel together.
    pub(crate) fn flush_all_caches(&self) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::FlushAllCaches)
    }

    /// Request cancellation of the in-flight run. Coarse: the running node
    /// finishes, but no further nodes are scheduled (a shared atomic the
    /// executor polls — no command-channel round-trip).
    pub(crate) fn cancel_run(&self) {
        self.worker.request_cancel();
    }

    /// Start the installed program's event loop, firing each emitter's events
    /// and executing their subscribers.
    pub(crate) fn start_event_loop(&self) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::StartEventLoop)
    }

    /// Stop the event loop (aborts the per-event tasks).
    pub(crate) fn stop_event_loop(&self) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::StopEventLoop)
    }

    /// Non-blocking drain of everything the worker has posted since the
    /// last frame.
    pub(crate) fn drain(&self) -> impl Iterator<Item = WorkerReport> + '_ {
        self.rx.try_iter()
    }
}

impl Drop for WorkerBridge {
    fn drop(&mut self) {
        if let Err(error) = self.runtime.block_on(self.worker.exit()) {
            tracing::error!(%error, "worker task failed during shutdown");
        }
    }
}

#[cfg(test)]
mod tests;
