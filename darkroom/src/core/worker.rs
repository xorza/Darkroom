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
use scenarium::NodeId;
use scenarium::{RunSeeds, Worker, WorkerMessage, WorkerReport};

use crate::core::background_runtime::BackgroundRuntime;
use crate::core::wake::Wake;

pub(crate) struct WorkerBridge {
    worker: Worker,
    rx: Receiver<WorkerReport>,
    runtime: BackgroundRuntime,
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
            worker,
            rx,
            runtime,
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

    /// Install the current program and evict an authored node's cache cone as
    /// one worker commit. Stopping the event loop keeps it from immediately
    /// repopulating the entries being removed.
    pub(crate) fn install_and_evict_cache(&self, compiled: Arc<CompiledGraph>, node_id: NodeId) {
        let _ = self.worker.send_many([
            WorkerMessage::Update { compiled },
            WorkerMessage::EvictCache {
                nodes: vec![node_id],
            },
            WorkerMessage::StopEventLoop,
        ]);
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

impl Drop for WorkerBridge {
    fn drop(&mut self) {
        if let Err(error) = self.runtime.block_on(self.worker.exit()) {
            tracing::error!(%error, "worker task failed during shutdown");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use scenarium::{
        Binding, Compiler, Graph, InputPort, StaticValue, WorkerActivity, WorkerReport,
        WorkerStatusKind, system_library, worker_events_library,
    };

    use crate::core::wake::Wake;
    use crate::core::worker::WorkerBridge;

    #[test]
    fn drop_waits_for_worker_idle_before_runtime_shutdown() {
        let wake_count = Arc::new(AtomicUsize::new(0));
        let wake: Wake = {
            let wake_count = Arc::clone(&wake_count);
            Arc::new(move || {
                wake_count.fetch_add(1, Ordering::SeqCst);
            })
        };
        let bridge = WorkerBridge::new(wake);

        let mut library = system_library();
        library.merge(worker_events_library());
        let mut graph = Graph::default();
        let frame = graph.add(library.by_name("Frame Event").unwrap().into());
        let print = graph.add(library.by_name("Print").unwrap().into());
        graph.set_input_binding(
            InputPort::new(frame, 0),
            Binding::from(StaticValue::Float(0.0)),
        );
        graph.set_input_binding(
            InputPort::new(print, 0),
            Binding::from(StaticValue::String("tick".to_string())),
        );
        graph.subscribe(frame, 1, print);
        let compiled = Compiler::default()
            .compile(&graph, &library)
            .unwrap()
            .into();
        bridge.install(compiled);
        bridge.start_event_loop();

        loop {
            let report = bridge
                .rx
                .recv_timeout(Duration::from_secs(5))
                .expect("worker did not start its event loop");
            if matches!(
                report,
                WorkerReport::Status(status)
                    if status.activity == WorkerActivity::EventLoop
                        && matches!(status.kind, WorkerStatusKind::Completed { .. })
            ) {
                break;
            }
        }
        let before_drop = wake_count.load(Ordering::SeqCst);

        drop(bridge);

        assert_eq!(wake_count.load(Ordering::SeqCst), before_drop + 1);
    }
}
