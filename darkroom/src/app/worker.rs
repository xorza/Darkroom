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
use scenarium::execution::{ArgumentValues, Error as ExecError};
use scenarium::execution_stats::ExecutionStats;
use scenarium::prelude::{FuncLib, Graph};
use scenarium::worker::{Worker, WorkerMessage};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;

use crate::run_state::ValueRequest;

/// A result delivered from the worker thread back to the frame loop.
/// Mirrors the worker's callback surface; richer variants (per-node
/// argument values) land here as the value/status surface grows.
#[derive(Debug)]
pub(crate) enum WorkerEvent {
    ExecutionFinished(Result<ExecutionStats, ExecError>),
    /// Reply to a [`WorkerBridge::request_argument_values`] call. The
    /// `request` echoes back the node + run epoch it was tagged with so a
    /// reply from a superseded run can be dropped. `values` is `None` when
    /// the worker couldn't resolve the node (e.g. it isn't in the executed
    /// program).
    ArgumentValues {
        request: ValueRequest,
        values: Option<ArgumentValues>,
    },
}

pub(crate) struct WorkerBridge {
    /// Kept alive so the worker's spawned tasks keep running; dropping it
    /// shuts the runtime down. Also used to spawn the forwarder that turns
    /// an argument-value oneshot reply into a [`WorkerEvent`].
    runtime: Runtime,
    worker: Worker,
    rx: Receiver<WorkerEvent>,
    /// Cloned into each forwarder task (and the execution callback) so
    /// replies reach the frame loop on the same channel.
    tx: Sender<WorkerEvent>,
    /// Cloned into forwarders to poke a repaint when a reply lands.
    host: HostHandle,
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
            let tx = tx.clone();
            let host = host.clone();
            Worker::new(move |result| Self::deliver(&tx, &host, result))
        };
        Self {
            runtime,
            worker,
            rx,
            tx,
            host,
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

    /// Ask the worker for one node's computed input/output values. The
    /// worker answers on a oneshot against its live executor slots; a
    /// forwarder task on our runtime turns that into a
    /// [`WorkerEvent::ArgumentValues`] on the frame channel (echoing the
    /// `request` so a stale-epoch reply can be dropped) and pokes a
    /// repaint. A dropped send (worker exited) just drops the forwarder —
    /// no reply ever arrives, which is the right shutdown behavior.
    pub(crate) fn request_argument_values(&self, request: ValueRequest) {
        let (reply_tx, reply_rx) = oneshot::channel();
        if self
            .worker
            .send(WorkerMessage::RequestArgumentValues {
                node_id: request.node_id,
                reply: reply_tx,
            })
            .is_err()
        {
            return;
        }
        let tx = self.tx.clone();
        let host = self.host.clone();
        self.runtime.spawn(async move {
            // `Err` means the worker dropped the reply end (shutdown /
            // graph cleared mid-flight); nothing to forward.
            if let Ok(values) = reply_rx.await {
                let _ = tx.send(WorkerEvent::ArgumentValues { request, values });
                host.request_repaint();
            }
        });
    }

    /// Non-blocking drain of everything the worker has posted since the
    /// last frame.
    pub(crate) fn drain(&self) -> impl Iterator<Item = WorkerEvent> + '_ {
        self.rx.try_iter()
    }
}
