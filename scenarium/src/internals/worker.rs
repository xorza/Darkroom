//! [`TestWorker`]: a [`TestGraph`] behind a live [`Worker`], with every answer
//! coming back **by name**.
//!
//! The worker speaks compiled artifacts and a callback stream of
//! [`WorkerReport`]s; a test speaks "install the graph, fire Frame Event, what
//! did it log". Bridging that per test is what produced a fixture struct, two
//! graph builders, a completed-runs-only channel and four
//! await-with-timeout helpers at the top of the worker's test file.
//!
//! Here the whole report stream is captured — installs, activity changes, live
//! patches, errors, completions — and a test picks out the reports it is about
//! with [`report`](TestWorker::report), [`status`](TestWorker::status) or
//! [`finished`](TestWorker::finished). Nothing is filtered on the way in, so a
//! test that starts caring about progress does not need a different fixture.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, timeout};

use crate::ConstValue;
use crate::builtin::system_library::system_library;
use crate::builtin::worker_events_library::worker_events_library;
use crate::execution::cache::disk_store::DiskStore;
use crate::execution::compile::compiled_graph::CompiledGraph;
use crate::execution::error::Error;
use crate::graph::identity::NodeId;
use crate::internals::engine::RunOutcome;
use crate::internals::graph::TestGraph;
use crate::worker::Worker;
use crate::worker::activity::WorkerActivity;
use crate::worker::error::WorkerError;
use crate::worker::protocol::{WorkerMessage, WorkerReport};

/// How long a wait here gives the worker before failing the test — generous
/// enough that a loaded machine does not flake, bounded so a wedged worker
/// fails instead of hanging the suite.
pub(crate) const PATIENCE: Duration = Duration::from_secs(5);

/// A [`TestGraph`] with a live worker over it.
#[derive(Debug)]
pub(crate) struct TestWorker {
    pub(crate) graph: TestGraph,
    pub(crate) worker: Worker,
    reports: mpsc::UnboundedReceiver<WorkerReport>,
    /// The last program the stream reported installed — what a completion
    /// arriving after it ran.
    installed: Option<Arc<CompiledGraph>>,
}

impl TestWorker {
    /// A worker over `graph`, with nothing installed yet.
    ///
    /// Installing is a message like any other, and several tests are about
    /// what a worker holding no graph does, so the first batch is always the
    /// test's to state.
    pub(crate) fn over(graph: TestGraph) -> Self {
        let (tx, reports) = mpsc::unbounded_channel();
        let worker = Worker::new(move |report| {
            // The receiver outlives the worker in every test here; a closed
            // channel means the fixture is being torn down.
            #[expect(
                clippy::unused_result_ok,
                reason = "a report sent during teardown has no reader"
            )]
            tx.send(report).ok();
        });
        Self {
            graph,
            worker,
            reports,
            installed: None,
        }
    }

    /// The frame fixture: `Frame Event` → `To String` → `Print`, over the real
    /// `system` and `worker_events` libraries, with the event's period bound to
    /// 1 and `Print` subscribed to it.
    ///
    /// Its funcs are production ones — the frame counter really counts and
    /// `Print` really logs — so successive event runs log `["1"]`, `["2"]`, …
    /// That *is* the point: this is the graph the worker's event path is
    /// exercised through.
    pub(crate) fn frames() -> Self {
        let mut library = system_library();
        library.merge(worker_events_library());

        let mut graph = TestGraph::over(library);
        graph.add_declared("Frame Event");
        graph.add_declared("To String");
        graph.add_declared("Print");
        graph.constant("Frame Event", 0, ConstValue::Int(1));
        graph.subscribe("Frame Event", 0, "Print");
        graph.wire("Frame Event", 1, "To String", 0);
        graph.wire("To String", 0, "Print", 0);
        Self::over(graph)
    }

    /// One `Print` sink bound to `message` — the minimal graph for a test
    /// about the worker rather than about a graph.
    pub(crate) fn printing(message: &str) -> Self {
        Self::over(Self::print_graph(message))
    }

    /// The graph [`printing`](Self::printing) is built over, for a test that
    /// installs a *second* one over the same worker.
    pub(crate) fn print_graph(message: &str) -> TestGraph {
        let mut graph = TestGraph::over(system_library());
        graph.add_declared("Print");
        graph.constant("Print", 0, ConstValue::String(message.to_owned()));
        graph
    }

    /// A fresh worker over the same graph, dropping this one and everything it
    /// held in RAM — the reopen a persistence test is about.
    pub(crate) fn restart(self) -> Self {
        Self::over(self.graph)
    }

    pub(crate) fn id(&self, name: &str) -> NodeId {
        self.graph.id(name)
    }

    /// `Update` carrying this graph as it stands — sent again after an edit.
    pub(crate) fn update(&self) -> WorkerMessage {
        WorkerMessage::Update {
            compiled: self.graph.program(),
        }
    }

    /// `Run` seeded with every sink — the "compute the document" entry.
    pub(crate) fn sinks() -> WorkerMessage {
        WorkerMessage::RunSinks
    }

    /// An event port on a named node.
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

    /// The worker's activity as the next report that states it gives it — an activity change,
    /// or a completed run's resting activity — skipping everything else.
    pub(crate) async fn activity(&mut self) -> WorkerActivity {
        loop {
            match self.report().await {
                WorkerReport::Activity(activity) => return activity,
                WorkerReport::Completed(summary) => return summary.activity,
                _ => {}
            }
        }
    }

    /// The next finished run, or the error that ended it — skipping everything
    /// a run publishes on the way there.
    pub(crate) async fn finished(&mut self) -> Result<RunOutcome, Error> {
        loop {
            match self.report().await {
                WorkerReport::Completed(summary) => {
                    return Ok(RunOutcome::published(&self.graph, &summary));
                }
                WorkerReport::Error(WorkerError::Execution { error }) => return Err(error),
                WorkerReport::Installed { .. }
                | WorkerReport::Cleared
                | WorkerReport::Activity(_)
                | WorkerReport::Progress { .. }
                | WorkerReport::Error(
                    WorkerError::CacheEviction { .. } | WorkerError::CacheFlush { .. },
                ) => {}
            }
        }
    }

    /// [`finished`](Self::finished), for the runs a test expects to succeed.
    pub(crate) async fn run(&mut self) -> RunOutcome {
        self.finished().await.expect("the run succeeds")
    }

    /// The program the stream last reported installed — what the run a
    /// completion just described was executed against.
    pub(crate) fn installed(&self) -> &Arc<CompiledGraph> {
        self.installed
            .as_ref()
            .expect("nothing was installed before this point")
    }

    /// Take every report already queued, so the next wait sees only what
    /// happens from here. Discarded by most callers; a test asserting on what a
    /// finished worker left behind reads the returned batch.
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
    /// The worker executes a batch's run before it answers the batch's `Sync`,
    /// so every run the batch caused has reported by the time `settle` returns.
    /// Activity and patch statuses are not runs and do not count — the claim is
    /// that nothing *executed*, which is what "silent no-op" means.
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

    /// Assert that nothing has run since the last read, and that nothing is about
    /// to: a bare [`settle_without_run`](Self::settle_without_run).
    pub(crate) async fn assert_no_run(&mut self) {
        self.settle_without_run(None::<WorkerMessage>).await;
    }
}
