//! The bridge between a frontend's `RuntimeHost` and the headless
//! graph-evaluation `Worker` (`scenarium::worker`). The worker runs on its
//! own tokio runtime; this type owns that runtime, the worker handle, and a
//! sync channel the worker's callback posts results onto. The host loop
//! (`App::update` / `TerminalSession::tick`) drains the channel each frame and is
//! woken from off-thread via the [`Wake`] callback.
//!
//! Outbound commands retain FIFO order, with program installation separate
//! from execution; inbound is a plain `std::sync::mpsc` because the consumer
//! is the synchronous frame loop on the main thread.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use scenarium::CompiledGraph;
use scenarium::DiskStore;
use scenarium::ExecutionNodeId;
use scenarium::{RunSeeds, Worker, WorkerMessage, WorkerReport};

use crate::core::background_runtime::BackgroundRuntime;
use crate::core::wake::Wake;

pub(crate) struct WorkerBridge {
    /// Kept alive so the worker's spawned tasks keep running; dropping it
    /// shuts the runtime down. Never read — its lifetime is the point.
    #[allow(dead_code)]
    runtime: BackgroundRuntime,
    worker: Worker,
    rx: Receiver<WorkerReport>,
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
        let (tx, rx) = channel::<WorkerReport>();
        // `Worker`'s `tokio::spawn` needs an ambient runtime.
        let worker = runtime.enter(|| Worker::new(move |report| Self::deliver(&tx, &wake, report)));
        Self {
            runtime,
            worker,
            rx,
        }
    }

    fn deliver(tx: &Sender<WorkerReport>, wake: &Wake, report: WorkerReport) {
        let _ = tx.send(report);
        (wake)();
    }

    /// Install a compiled program. The worker acknowledges it with
    /// `WorkerReport::Installed` before processing reports from later commands.
    pub(crate) fn install(&self, compiled: Arc<CompiledGraph>) {
        let _ = self.worker.send(WorkerMessage::Update { compiled });
    }

    /// Execute every sink in the installed program.
    pub(crate) fn run_sinks(&self) {
        let _ = self.worker.send(WorkerMessage::Run {
            seeds: RunSeeds::sinks(),
        });
    }

    /// Execute one exact node in the installed program and deliver its outputs.
    pub(crate) fn run_node(&self, e_node_id: ExecutionNodeId) {
        let _ = self.worker.send(WorkerMessage::Run {
            seeds: RunSeeds::nodes(vec![e_node_id]),
        });
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

    /// Start the installed program's event loop, firing each emitter's events
    /// and executing their subscribers.
    pub(crate) fn start_event_loop(&self) {
        let _ = self.worker.send(WorkerMessage::StartEventLoop);
    }

    /// Stop the event loop (aborts the per-event tasks). A dropped send
    /// (worker already exited) is a harmless shutdown no-op.
    pub(crate) fn stop_event_loop(&self) {
        let _ = self.worker.send(WorkerMessage::StopEventLoop);
    }

    /// Non-blocking drain of everything the worker has posted since the
    /// last frame.
    pub(crate) fn drain(&self) -> impl Iterator<Item = WorkerReport> + '_ {
        self.rx.try_iter()
    }
}
