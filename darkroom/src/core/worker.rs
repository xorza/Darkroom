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

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use scenarium::CompiledGraph;
use scenarium::DiskStore;
use scenarium::NodeId;
use scenarium::{RunSeeds, Worker, WorkerExited, WorkerMessage, WorkerReport};

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
    ///
    /// Every command here answers whether the worker took it. A send only
    /// fails once the worker task is gone, and then *no report will ever
    /// arrive* — so a caller that assumed success would leave the UI
    /// waiting on a run that can never report back.
    pub(crate) fn install(&self, compiled: Arc<CompiledGraph>) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::Update { compiled })
    }

    /// Drop the installed program and everything its cache holds. The worker
    /// acknowledges it with `WorkerReport::Cleared`.
    ///
    /// Best-effort like the event-loop stop beside it: a worker that has
    /// already exited holds nothing to release, which is the outcome the
    /// caller wanted.
    pub(crate) fn clear(&self) {
        let _ = self.worker.send(WorkerMessage::Clear);
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
        self.worker.send(WorkerMessage::Run {
            seeds: RunSeeds::sinks(),
        })
    }

    /// Execute these exact nodes in the installed program and deliver their
    /// outputs. Plural because a run is seeded with a set — a "run to this
    /// node" contributes one.
    pub(crate) fn run_nodes(&self, node_ids: Vec<NodeId>) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::Run {
            seeds: RunSeeds::nodes(node_ids),
        })
    }

    /// Swap the engine's output cache (codec registry + store root) — e.g. to
    /// repoint at the active document's store. Takes effect before the next
    /// run's compile. Attaching writes nothing; see [`Self::flush_all_caches`].
    /// No caller is waiting on an answer, so the outcome is
    /// traced rather than returned; a run is where a dead worker becomes the
    /// user's problem.
    pub(crate) fn set_disk_store(&self, cache: DiskStore) {
        if self
            .worker
            .send(WorkerMessage::SetDiskStore(cache))
            .is_err()
        {
            tracing::warn!("worker exited; disk store not installed");
        }
    }

    /// Write every installed node's resident disk-backed value into the
    /// attached store — what the store is owed by values computed while there
    /// was nowhere to put them. Ordered after any `SetDiskStore` in the same
    /// batch by the worker's apply order, so the two travel together.
    ///
    /// Traced rather than returned, like the attach above: nothing waits on it.
    pub(crate) fn flush_all_caches(&self) {
        if self.worker.send(WorkerMessage::FlushAllCaches).is_err() {
            tracing::warn!("worker exited; resident values not flushed");
        }
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
    use std::time::{Duration, Instant};

    use scenarium::{
        Binding, Compiler, ConstValue, Graph, InputPort, WorkerActivity, WorkerReport,
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
            Binding::from(ConstValue::Float(0.0)),
        );
        graph.set_input_binding(
            InputPort::new(print, 0),
            Binding::from(ConstValue::String("tick".to_string())),
        );
        graph.subscribe(frame, 1, print);
        let compiled = Compiler::default()
            .compile(&graph, &library)
            .unwrap()
            .into();
        bridge
            .install(compiled)
            .expect("worker accepts the program");
        bridge
            .start_event_loop()
            .expect("worker accepts the event-loop start");

        let mut delivered = 0_usize;
        loop {
            let report = bridge
                .rx
                .recv_timeout(Duration::from_secs(5))
                .expect("worker did not start its event loop");
            delivered += 1;
            if matches!(
                report,
                WorkerReport::Status(status)
                    if status.activity == WorkerActivity::EventLoop
                        && matches!(status.kind, WorkerStatusKind::Completed { .. })
            ) {
                break;
            }
        }
        // A report is queued *before* its wake fires, so receiving one says
        // nothing about its wake having landed yet. Settling the count against
        // the reports actually taken is what makes the drop delta below mean
        // the shutdown wake rather than a straggler from the run.
        //
        // It does settle: the loop is quiescent — the FPS event is disabled at
        // 0 Hz, so its lambda parks forever and no further report is produced
        // until shutdown.
        let settle_by = Instant::now() + Duration::from_secs(5);
        while wake_count.load(Ordering::SeqCst) < delivered {
            assert!(
                Instant::now() < settle_by,
                "only {} of {delivered} delivered reports woke the host",
                wake_count.load(Ordering::SeqCst),
            );
            std::thread::yield_now();
        }
        assert_eq!(
            wake_count.load(Ordering::SeqCst),
            delivered,
            "every wake belongs to a report the host was handed"
        );

        drop(bridge);

        assert_eq!(
            wake_count.load(Ordering::SeqCst),
            delivered + 1,
            "shutdown reports the worker idle, and wakes the host for it, before the runtime goes away"
        );
    }

    #[test]
    fn commands_report_a_dead_worker_instead_of_reading_as_queued() {
        // A send only fails once the worker task is gone, and then no
        // report can ever arrive — so a caller told "queued" would wait
        // forever on a run that will never report back.
        let mut bridge = WorkerBridge::new(Arc::new(|| {}));
        assert!(bridge.run_sinks().is_ok(), "a live worker takes commands");

        // Stop the worker the way shutdown does, then keep issuing commands.
        bridge
            .runtime
            .block_on(bridge.worker.exit())
            .expect("worker task joins cleanly");

        assert!(
            bridge.run_sinks().is_err(),
            "a run command to a dead worker must not read as accepted"
        );
        assert!(
            bridge.start_event_loop().is_err(),
            "nor an event-loop start"
        );
    }
}
