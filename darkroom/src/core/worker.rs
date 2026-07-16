//! The bridge between a frontend's `Engine` and the headless
//! graph-evaluation `Worker` (`scenarium::worker`). The worker runs on its
//! own tokio runtime; this type owns that runtime, the worker handle, and a
//! sync channel the worker's callback posts results onto. The host loop
//! (`App::update` / `Session::tick`) drains the channel each frame and is
//! woken from off-thread via the [`Wake`] callback.
//!
//! Outbound is one batched send per request (the worker scans a batch
//! into a single commit); inbound is a plain `std::sync::mpsc` because
//! the consumer is the synchronous frame loop on the main thread.

use std::sync::mpsc::{Receiver, Sender, channel};

use scenarium::CompiledGraph;
use scenarium::DiskStore;
use scenarium::Error as ExecError;
use scenarium::NodeAddress;
use scenarium::{ExecutionStats, PinnedOutputs, RunProgress};
use scenarium::{Worker, WorkerMessage, WorkerReport};

use crate::core::background_runtime::BackgroundRuntime;
use crate::core::wake::Wake;

/// A result delivered from the worker thread back to the frame loop.
/// Mirrors the worker's callback surface.
#[derive(Debug)]
pub(crate) enum WorkerEvent {
    ExecutionFinished(Result<ExecutionStats, ExecError>),
    /// Live per-node progress during a run, ahead of `ExecutionFinished`.
    NodeProgress(RunProgress),
    /// A pinned output (or pinned-root node) just produced a fresh value,
    /// pushed right after its node finished running — ahead of
    /// `ExecutionFinished`, like `NodeProgress`.
    PinnedOutputs(PinnedOutputs),
}

pub(crate) struct WorkerBridge {
    /// Kept alive so the worker's spawned tasks keep running; dropping it
    /// shuts the runtime down. Never read — its lifetime is the point.
    #[allow(dead_code)]
    runtime: BackgroundRuntime,
    worker: Worker,
    rx: Receiver<WorkerEvent>,
}

impl std::fmt::Debug for WorkerBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        let (tx, rx) = channel::<WorkerEvent>();
        // `Worker`'s `tokio::spawn` needs an ambient runtime.
        let worker = runtime.enter(|| Worker::new(move |report| Self::deliver(&tx, &wake, report)));
        Self {
            runtime,
            worker,
            rx,
        }
    }

    fn deliver(tx: &Sender<WorkerEvent>, wake: &Wake, report: WorkerReport) {
        let event = match report {
            WorkerReport::Progress(progress) => WorkerEvent::NodeProgress(progress),
            WorkerReport::PinnedOutputs(pinned) => WorkerEvent::PinnedOutputs(pinned),
            WorkerReport::Finished(result) => WorkerEvent::ExecutionFinished(result),
        };
        let _ = tx.send(event);
        (wake)();
    }

    /// Run the compiled program once: install it on the worker, then
    /// execute its sinks. One batched send so the worker commits
    /// both as a unit. A dropped send (worker already exited) is a
    /// harmless shutdown no-op.
    pub(crate) fn run_once(&self, compiled: CompiledGraph) {
        let _ = self.worker.send_many([
            WorkerMessage::Update { compiled },
            WorkerMessage::ExecuteSinks,
        ]);
    }

    /// Run one node's upstream cone: install the compiled program, then
    /// execute with the node as seed, keeping its outputs resident. One
    /// batched send so the seed always targets the program it was compiled
    /// against.
    pub(crate) fn run_node(&self, compiled: CompiledGraph, node: NodeAddress) {
        let _ = self.worker.send_many([
            WorkerMessage::Update { compiled },
            WorkerMessage::ExecuteNodes { nodes: vec![node] },
        ]);
    }

    /// Flush resident cache values to disk **without running the graph** — e.g. after
    /// a node's disk-cache toggle, so its in-RAM value is persisted now rather than
    /// waiting for the next run (a cache-hit node never re-executes to store itself).
    pub(crate) fn save_caches(&self, compiled: CompiledGraph) {
        let _ = self.worker.send(WorkerMessage::SaveCaches { compiled });
    }

    /// Swap the engine's output cache (codec registry + store root) — e.g. to
    /// repoint at the active document's store. Takes effect before
    /// the next run's compile. A dropped send (worker exited) is a harmless no-op.
    pub(crate) fn set_disk_store(&self, cache: DiskStore) {
        let _ = self.worker.send(WorkerMessage::SetDiskStore(cache));
    }

    /// Request cancellation of the in-flight run. Coarse: the running node
    /// finishes, but no further nodes are scheduled (a shared atomic the
    /// executor polls — no command-channel round-trip).
    pub(crate) fn cancel_run(&self) {
        self.worker.request_cancel();
    }

    /// Start the event loop on the compiled program: install it, then run the
    /// loop that fires each emitter's events and executes their subscribers.
    /// One batched send so the `Update` and `StartEventLoop` commit as a unit.
    pub(crate) fn start_event_loop(&self, compiled: CompiledGraph) {
        let _ = self.worker.send_many([
            WorkerMessage::Update { compiled },
            WorkerMessage::StartEventLoop,
        ]);
    }

    /// Stop the event loop (aborts the per-event tasks). A dropped send
    /// (worker already exited) is a harmless shutdown no-op.
    pub(crate) fn stop_event_loop(&self) {
        let _ = self.worker.send(WorkerMessage::StopEventLoop);
    }

    /// Non-blocking drain of everything the worker has posted since the
    /// last frame.
    pub(crate) fn drain(&self) -> impl Iterator<Item = WorkerEvent> + '_ {
        self.rx.try_iter()
    }
}
