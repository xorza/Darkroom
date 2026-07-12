//! The bridge between a frontend's `Engine` and the headless
//! graph-evaluation `Worker` (`scenarium::worker`). The worker runs on its
//! own tokio runtime; this type owns that runtime, the worker handle, and a
//! sync channel the worker's callback posts results onto. The host loop
//! (`App::frame` / `Session::tick`) drains the channel each frame and is
//! woken from off-thread via the [`Wake`] callback.
//!
//! Outbound is one batched send per request (the worker scans a batch
//! into a single commit); inbound is a plain `std::sync::mpsc` because
//! the consumer is the synchronous frame loop on the main thread.

use std::sync::mpsc::{Receiver, Sender, channel};

use scenarium::execution::compile::CompiledGraph;
use scenarium::execution::disk_store::DiskStore;
use scenarium::execution::stats::{ExecutionStats, RunProgress};
use scenarium::execution::{ArgumentValues, Error as ExecError};
use scenarium::graph::NodeId;
use scenarium::worker::{Worker, WorkerMessage, WorkerReport};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;

use crate::core::wake::Wake;

/// Run epoch. Bumped each time a frontend kicks a fresh run so values from
/// a superseded run can be dropped on arrival.
pub(crate) type RunId = u64;

/// One node's pending value fetch: which node, tagged with the run epoch
/// it was requested under (echoed back on the reply so a stale-run reply
/// can be dropped). The worker's request/reply currency, so it lives here
/// rather than in the GUI's `run_state`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ValueRequest {
    pub(crate) node_id: NodeId,
    pub(crate) run_id: RunId,
}

/// A result delivered from the worker thread back to the frame loop.
/// Mirrors the worker's callback surface; richer variants (per-node
/// argument values) land here as the value/status surface grows.
#[derive(Debug)]
pub(crate) enum WorkerEvent {
    ExecutionFinished(Result<ExecutionStats, ExecError>),
    /// Live per-node progress during a run, ahead of `ExecutionFinished`.
    NodeProgress(RunProgress),
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
    /// Cloned into forwarders to wake the host loop when a reply lands.
    wake: Wake,
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
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build worker runtime");
        let (tx, rx) = channel::<WorkerEvent>();
        // `Worker`'s `tokio::spawn` needs an ambient runtime.
        let worker = {
            let _guard = runtime.enter();
            let tx = tx.clone();
            let wake = wake.clone();
            Worker::new(move |report| Self::deliver(&tx, &wake, report))
        };
        Self {
            runtime,
            worker,
            rx,
            tx,
            wake,
        }
    }

    fn deliver(tx: &Sender<WorkerEvent>, wake: &Wake, report: WorkerReport) {
        let event = match report {
            WorkerReport::Progress(progress) => WorkerEvent::NodeProgress(progress),
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
    /// execute with the node as seed — its outputs stay resident for
    /// the preview value fetch. One batched send so the seed always
    /// targets the program it was compiled against.
    pub(crate) fn run_node(&self, compiled: CompiledGraph, node_id: NodeId) {
        let _ = self.worker.send_many([
            WorkerMessage::Update { compiled },
            WorkerMessage::ExecuteNodes {
                nodes: vec![node_id],
            },
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

    /// Ask the worker for several nodes' computed input/output values —
    /// one batched send so they commit against the same executor snapshot
    /// (rather than one `send` per node racing separate worker-loop ticks).
    /// Each still answers on its own oneshot against the worker's live
    /// executor slots; a forwarder task per request turns its reply into a
    /// [`WorkerEvent::ArgumentValues`] on the frame channel (echoing the
    /// `request` so a stale-epoch reply can be dropped) and pokes a
    /// repaint. A dropped send (worker exited) just drops the forwarders —
    /// no reply ever arrives, which is the right shutdown behavior.
    pub(crate) fn request_argument_values(&self, requests: impl IntoIterator<Item = ValueRequest>) {
        let (msgs, replies): (Vec<_>, Vec<_>) = requests
            .into_iter()
            .map(|request| {
                let (reply_tx, reply_rx) = oneshot::channel();
                (
                    WorkerMessage::RequestArgumentValues {
                        node_id: request.node_id,
                        reply: reply_tx,
                    },
                    (request, reply_rx),
                )
            })
            .unzip();
        if replies.is_empty() || self.worker.send_many(msgs).is_err() {
            return;
        }
        for (request, reply_rx) in replies {
            let tx = self.tx.clone();
            let wake = self.wake.clone();
            self.runtime.spawn(async move {
                // `Err` means the worker dropped the reply end (shutdown /
                // graph cleared mid-flight); nothing to forward.
                if let Ok(values) = reply_rx.await {
                    let _ = tx.send(WorkerEvent::ArgumentValues { request, values });
                    (wake)();
                }
            });
        }
    }

    /// Non-blocking drain of everything the worker has posted since the
    /// last frame.
    pub(crate) fn drain(&self) -> impl Iterator<Item = WorkerEvent> + '_ {
        self.rx.try_iter()
    }
}
