//! The bridge between `App` and the headless graph-evaluation `Worker`
//! (`scenarium::worker`). The worker runs on its own tokio runtime; this
//! type owns that runtime, the worker handle, and a sync channel the
//! worker's callback posts results onto. `App` drains the channel each
//! frame (`drain_worker_events`) and pokes a repaint from off-thread via
//! [`HostHandle::request_repaint`].
//!
//! Outbound is one batched send per request (the worker scans a batch
//! into a single commit); inbound is a plain `std::sync::mpsc` because
//! the consumer is the synchronous frame loop on the main thread.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use palantir::HostHandle;
use scenarium::execution::Error as ExecError;
use scenarium::execution_stats::ExecutionStats;
use scenarium::prelude::{FuncLib, Graph};
use scenarium::worker::{Worker, WorkerMessage};
use tokio::runtime::Runtime;

/// A result delivered from the worker thread back to the frame loop.
/// Mirrors the worker's callback surface; richer variants (per-node
/// argument values) land here as the value/status surface grows.
#[derive(Debug)]
pub(crate) enum WorkerEvent {
    ExecutionFinished(Result<ExecutionStats, ExecError>),
}

pub(crate) struct WorkerBridge {
    /// Kept alive so the worker's spawned tasks keep running; dropping
    /// it shuts the runtime down. Never touched after construction.
    _runtime: Runtime,
    worker: Worker,
    rx: Receiver<WorkerEvent>,
}

impl std::fmt::Debug for WorkerBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerBridge").finish_non_exhaustive()
    }
}

impl WorkerBridge {
    /// Spin up the worker on a fresh multi-thread runtime. The callback
    /// runs on a worker thread: it forwards the result over `tx` and
    /// asks the host to paint, so the next frame drains it.
    pub(crate) fn new(host: HostHandle) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build worker runtime");
        let (tx, rx) = channel::<WorkerEvent>();
        // `Worker::new`'s `tokio::spawn` needs an ambient runtime.
        let worker = {
            let _guard = runtime.enter();
            Worker::new(move |result| Self::deliver(&tx, &host, result))
        };
        Self {
            _runtime: runtime,
            worker,
            rx,
        }
    }

    fn deliver(
        tx: &Sender<WorkerEvent>,
        host: &HostHandle,
        result: Result<ExecutionStats, ExecError>,
    ) {
        let _ = tx.send(WorkerEvent::ExecutionFinished(result));
        host.request_repaint();
    }

    /// Run the graph once: replace the worker's graph + lib, then
    /// execute its terminals. One batched send so the worker commits
    /// both as a unit. A dropped send (worker already exited) is a
    /// harmless shutdown no-op.
    pub(crate) fn run_once(&self, graph: Graph, func_lib: Arc<FuncLib>) {
        let _ = self.worker.send_many([
            WorkerMessage::Update { graph, func_lib },
            WorkerMessage::ExecuteTerminals,
        ]);
    }

    /// Non-blocking drain of everything the worker has posted since the
    /// last frame.
    pub(crate) fn drain(&self) -> impl Iterator<Item = WorkerEvent> + '_ {
        self.rx.try_iter()
    }
}
