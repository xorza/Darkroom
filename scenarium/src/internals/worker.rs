//! [`TestWorker`]: a [`TestGraph`] behind a live [`Worker`], with every answer
//! coming back **by name**.
//!
//! The whole [`WorkerReport`] stream is captured, unfiltered, and a test picks out the reports
//! it is about with [`report`](TestWorker::report), [`activity`](TestWorker::activity) or
//! [`finished`](TestWorker::finished).

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, timeout};

use crate::builtin::system_library::system_library;
use crate::builtin::worker_events_library::worker_events_library;
use crate::data::const_value::ConstValue;
use crate::exec::cache::disk_store::DiskStore;
use crate::exec::compile::compiled_graph::CompiledGraph;
use crate::exec::error::ExecError;
use crate::graph::identity::NodeId;
use crate::internals::engine::RunOutcome;
use crate::internals::test_graph::TestGraph;
use crate::worker::Worker;
use crate::worker::activity::WorkerActivity;
use crate::worker::error::WorkerError;
use crate::worker::protocol::{WorkerMessage, WorkerReport};

/// How long a wait here gives the worker before failing the test: generous for a loaded
/// machine, bounded so a wedged worker fails instead of hanging the suite.
pub(crate) const PATIENCE: Duration = Duration::from_secs(5);

/// A [`TestGraph`] with a live worker over it.
#[derive(Debug)]
pub(crate) struct TestWorker {
    pub(crate) graph: TestGraph,
    pub(crate) worker: Worker,
    reports: mpsc::UnboundedReceiver<WorkerReport>,
    /// The last program the stream reported installed.
    installed: Option<Arc<CompiledGraph>>,
}

impl TestWorker {
    /// A worker over `graph`, with nothing installed yet: installing is a message like any other.
    pub(crate) fn over(graph: TestGraph) -> Self {
        let (tx, reports) = mpsc::unbounded_channel();
        let worker = Worker::new(move |report| {
            // A closed channel means the fixture is being torn down.
            tx.send(report).ok();
        });
        Self {
            graph,
            worker,
            reports,
            installed: None,
        }
    }

    /// The frame fixture: `Frame Event` → `To String` → `Print`, over the real `system` and
    /// `worker_events` libraries, with the event's period bound to 1 and `Print` subscribed to
    /// it, so successive event runs log `["1"]`, `["2"]`, …
    pub(crate) fn frames() -> Self {
        let mut library = system_library();
        library.merge(worker_events_library());

        let mut graph = TestGraph::over(library);
        graph.add_declared("Frame Event");
        graph.add_declared("To String");
        graph.add_declared("Print");
        graph.constant("Frame Event", 0, ConstValue::Int(1));
        graph.subscribe("Frame Event", 0, "Print");
        graph.wire(("Frame Event", 1), "To String");
        graph.wire("To String", "Print");
        Self::over(graph)
    }

    /// One `Print` sink bound to `message`.
    pub(crate) fn printing(message: &str) -> Self {
        Self::over(Self::print_graph(message))
    }

    /// The graph [`printing`](Self::printing) is built over.
    pub(crate) fn print_graph(message: &str) -> TestGraph {
        let mut graph = TestGraph::over(system_library());
        graph.add_declared("Print");
        graph.constant("Print", 0, ConstValue::String(message.to_owned()));
        graph
    }

    /// A fresh worker over the same graph, dropping this one and everything it held in RAM.
    pub(crate) fn restart(self) -> Self {
        Self::over(self.graph)
    }

    pub(crate) fn id(&self, name: &str) -> NodeId {
        self.graph.id(name)
    }

    /// `Update` carrying this graph as it stands.
    pub(crate) fn update(&self) -> WorkerMessage {
        WorkerMessage::Update {
            compiled: self.graph.program(),
        }
    }

    /// `Run` seeded with every sink.
    pub(crate) fn sinks() -> WorkerMessage {
        WorkerMessage::RunSinks
    }

    /// `Run` seeded with one firing of that event.
    pub(crate) fn fire(&self, name: &str, event_idx: usize) -> WorkerMessage {
        WorkerMessage::FireEvents {
            events: vec![self.graph.event(name, event_idx)],
        }
    }

    /// `SetDiskStore` pointing the cache's disk tier at `root`.
    pub(crate) fn disk_store(root: impl Into<PathBuf>) -> WorkerMessage {
        WorkerMessage::SetDiskStore(DiskStore::new(Some(root.into())))
    }

    pub(crate) fn send(&self, msg: WorkerMessage) {
        self.worker.send(msg).expect("the worker is still running");
    }

    pub(crate) fn send_many(&self, msgs: impl IntoIterator<Item = WorkerMessage>) {
        self.worker
            .send_many(msgs)
            .expect("the worker is still running");
    }

    /// Send `msgs` plus a trailing `Sync`, blocking until that batch has fully
    /// committed. An empty `msgs` is the bare round trip that proves the worker
    /// is alive and drained.
    pub(crate) async fn settle(&self, msgs: impl IntoIterator<Item = WorkerMessage>) {
        let (reply, ack) = oneshot::channel();
        self.send_many(msgs.into_iter().chain([WorkerMessage::Sync { reply }]));
        timeout(PATIENCE, ack)
            .await
            .expect("sync timed out")
            .expect("the worker dropped the acknowledgement");
    }

    /// The bare `Sync` round trip: the worker is alive and has drained
    /// everything sent before this call.
    pub(crate) async fn sync(&self) {
        self.settle(None::<WorkerMessage>).await;
    }

    /// The next report of any kind, recording an install as it passes.
    pub(crate) async fn report(&mut self) -> WorkerReport {
        let report = timeout(PATIENCE, self.reports.recv())
            .await
            .expect("the worker published nothing in time")
            .expect("the worker's report channel closed");
        self.observe(&report);
        report
    }

    /// Track which program the worker holds, from a report read off the stream.
    fn observe(&mut self, report: &WorkerReport) {
        match report {
            WorkerReport::Installed { compiled, .. } => self.installed = Some(Arc::clone(compiled)),
            WorkerReport::Cleared => self.installed = None,
            WorkerReport::Activity(_)
            | WorkerReport::Progress { .. }
            | WorkerReport::Completed(_)
            | WorkerReport::Error(_) => {}
        }
    }

    /// The worker's activity as the next report that states it gives it, an activity change or a
    /// completed run's resting activity.
    pub(crate) async fn activity(&mut self) -> WorkerActivity {
        loop {
            match self.report().await {
                WorkerReport::Activity(activity) => return activity,
                WorkerReport::Completed(summary) => return summary.activity,
                _ => {}
            }
        }
    }

    /// The next finished run, or the error that ended it.
    pub(crate) async fn finished(&mut self) -> Result<RunOutcome, ExecError> {
        loop {
            match self.report().await {
                WorkerReport::Completed(summary) => {
                    return Ok(RunOutcome::published(&self.graph, &summary));
                }
                WorkerReport::Error(WorkerError::Exec { error }) => return Err(error),
                WorkerReport::Installed { .. }
                | WorkerReport::Cleared
                | WorkerReport::Activity(_)
                | WorkerReport::Progress { .. }
                | WorkerReport::Error(
                    WorkerError::CacheEviction { .. }
                    | WorkerError::CacheFlush { .. }
                    | WorkerError::EventLambdaPanic { .. },
                ) => {}
            }
        }
    }

    /// [`finished`](Self::finished), for the runs a test expects to succeed.
    pub(crate) async fn run(&mut self) -> RunOutcome {
        self.finished().await.expect("the run succeeds")
    }

    /// The program the stream last reported installed.
    pub(crate) fn installed(&self) -> &Arc<CompiledGraph> {
        self.installed
            .as_ref()
            .expect("nothing was installed before this point")
    }

    /// Take every report already queued, so the next wait sees only what happens from here.
    pub(crate) fn drain(&mut self) -> Vec<WorkerReport> {
        let mut drained = Vec::new();
        while let Ok(report) = self.reports.try_recv() {
            self.observe(&report);
            drained.push(report);
        }
        drained
    }

    /// Assert the stream holds nothing the test has not already read.
    pub(crate) fn quiet(&mut self) {
        let extra = self.reports.try_recv();
        assert!(extra.is_err(), "unexpected worker report: {extra:?}");
    }

    /// [`settle`](Self::settle) `msgs`, then assert the batch completed no run.
    ///
    /// The worker executes a batch's run before it answers the batch's `Sync`, so every run the
    /// batch caused has reported by the time `settle` returns. Activity and progress reports are
    /// not runs and do not count.
    pub(crate) async fn settle_without_run(
        &mut self,
        msgs: impl IntoIterator<Item = WorkerMessage>,
    ) {
        self.settle(msgs).await;
        let runs: Vec<WorkerReport> = self
            .drain()
            .into_iter()
            .filter(|report| matches!(report, WorkerReport::Completed(_)))
            .collect();
        assert!(runs.is_empty(), "unexpected run: {runs:?}");
    }

    /// A bare [`settle_without_run`](Self::settle_without_run).
    pub(crate) async fn assert_no_run(&mut self) {
        self.settle_without_run(None::<WorkerMessage>).await;
    }
}
