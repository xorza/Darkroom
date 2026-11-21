use std::fmt;
use std::mem;
use std::sync::Arc;

use tokio::task;

use crate::exec::cache::runtime_cache::cache_flush_report::CacheFlushReport;
use crate::exec::engine::{ExecEngine, ExecuteRequest};
use crate::exec::report::exec_outcome::ExecOutcome;
use crate::exec::report::run_reporter::RunPhase;
use crate::exec::report::run_reporter::RunReporter;
use crate::identity::program_id::ProgramId;
use crate::worker::active_event_loop::ActiveEventLoop;
use crate::worker::activity::WorkerActivity;
use crate::worker::batch_intent::{BatchIntent, GraphOp, LoopCommand};
use crate::worker::error::WorkerError;
use crate::worker::mailbox::{Mailbox, Taken};
use crate::worker::protocol::WorkerReport;
use crate::worker::run_epoch::RunEpoch;
use crate::worker::run_summary::RunSummaryPublisher;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventLoopTransition {
    Preserve,
    Stop,
    Rebuild,
}

impl EventLoopTransition {
    const fn for_intent(intent: &BatchIntent, event_loop_active: bool) -> Self {
        match intent.loop_request {
            Some(LoopCommand::Start) => Self::Rebuild,
            Some(LoopCommand::Stop) => Self::Stop,
            None if event_loop_active && intent.graph_state.is_some() => Self::Rebuild,
            None => Self::Preserve,
        }
    }
}

/// Which halves of a flush report reach the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlushReporting {
    /// A flush the host asked for, by node. Everything it did not write is the answer, a type
    /// that will never persist included, since the node goes on showing itself disk-backed.
    Requested,
    /// The sweep a newly attached store owes every already-resident value. Nobody named these
    /// nodes, so an unpersistable type is a standing fact about the library; only a write that
    /// broke is raised.
    Sweep,
}

/// Whether stopping the event loop tells the host the worker went `Idle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdleReport {
    Report,
    /// For a stop made while applying an intent: a rebuild's `execute` reports `Executing`
    /// directly, with no transient `Idle` between.
    Quiet,
}

/// Where the worker's reports go: the host's callback, called in place on the worker task.
pub(crate) struct ReportSink(Box<dyn Fn(WorkerReport) + Send + Sync>);

impl ReportSink {
    pub(crate) fn new(callback: impl Fn(WorkerReport) + Send + Sync + 'static) -> Self {
        Self(Box::new(callback))
    }

    fn publish(&self, report: WorkerReport) {
        (self.0)(report);
    }
}

impl fmt::Debug for ReportSink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReportSink")
    }
}

#[derive(Debug)]
pub(crate) struct WorkerTask {
    mailbox: Arc<Mailbox>,
    reports: ReportSink,
    engine: ExecEngine,
    summary: RunSummaryPublisher,
    outcome: ExecOutcome,
    intent: BatchIntent,
    event_loop: Option<ActiveEventLoop>,
    epoch: RunEpoch,
}

/// A task that ends refuses every later post, however it ends.
impl Drop for WorkerTask {
    fn drop(&mut self) {
        self.mailbox.close();
    }
}

impl WorkerTask {
    pub(crate) fn new(mailbox: Arc<Mailbox>, reports: ReportSink) -> Self {
        Self {
            mailbox,
            reports,
            engine: ExecEngine::default(),
            summary: RunSummaryPublisher::default(),
            outcome: ExecOutcome::default(),
            intent: BatchIntent::default(),
            event_loop: None,
            epoch: RunEpoch::default(),
        }
    }

    pub(crate) async fn run(mut self) {
        while self.next_batch().await == Taken::Batch {
            self.apply_intent().await;
            self.epoch.advance();
            task::yield_now().await;
        }
        self.stop_event_loop().await;
    }

    /// Wait for the next batch, or the shutdown. An event task's panic is resumed here, ahead of
    /// a post: the loop's other sources post once per batch, so a post is ready at nearly every
    /// wake and would keep the panic waiting for good.
    async fn next_batch(&mut self) -> Taken {
        loop {
            tokio::select! {
                biased;
                never = async {
                    self.event_loop.as_mut().unwrap().failed().await
                }, if self.event_loop.is_some() => match never {},
                () = self.mailbox.posted() => {}
            }
            self.intent.clear();
            match self.mailbox.take_into(&mut self.intent) {
                Taken::Nothing => {}
                taken => return taken,
            }
        }
    }

    async fn apply_intent(&mut self) {
        let transition = EventLoopTransition::for_intent(&self.intent, self.event_loop.is_some());
        let ends_loop = transition != EventLoopTransition::Preserve;
        let stopped_loop = ends_loop && self.event_loop.is_some();
        if ends_loop {
            self.intent.void_events();
        }
        if stopped_loop {
            self.finish_event_loop(IdleReport::Quiet).await;
        }

        // The graph op leads: every stage below is about the program it establishes. After the
        // store, the store's sweep wrote the outgoing document's values into the incoming
        // document's root; see [`BatchIntent`].
        match self.intent.graph_state.take() {
            Some(GraphOp::Clear) => {
                self.engine.clear();
                self.reports.publish(WorkerReport::Cleared);
            }
            Some(GraphOp::Replace(compiled)) => {
                self.engine.install(Arc::clone(&compiled));
                // After the install: reconciling onto the new program releases the slots of the
                // nodes it dropped.
                let cache_ram = self.engine.resident_cache_ram();
                self.reports.publish(WorkerReport::Installed {
                    compiled,
                    cache_ram,
                });
            }
            None => {}
        }

        if let Some(cache) = self.intent.disk_store.take() {
            self.engine.set_disk_store(cache);
        }
        let failures = self.engine.prune_store().await;
        if !failures.is_empty() {
            self.reports
                .publish(WorkerReport::Error(WorkerError::CachePrune { failures }));
        }
        if let Some(bytes) = self.intent.ram_headroom.take() {
            self.engine.set_ram_headroom(bytes);
        }

        self.evict_cache().await;
        self.flush_cache().await;

        // Rebuilding the loop re-initializes every event source, whether or not a message asked.
        if transition == EventLoopTransition::Rebuild {
            self.intent.seeds.event_sources = true;
        }
        let ran = !self.intent.seeds.is_empty() && !self.engine.is_empty();
        if ran {
            self.execute(transition).await;
        }
        // A rebuild's `execute` reports `Executing` directly, so only a stop with no follow-up run
        // is idle.
        if stopped_loop && !ran {
            self.reports
                .publish(WorkerReport::Activity(WorkerActivity::Idle));
        }

        #[cfg(test)]
        for reply in self.intent.syncs.drain(..) {
            reply.send(()).ok();
        }
    }

    async fn evict_cache(&mut self) {
        if self.intent.evict_cache.is_empty() {
            return;
        }

        // `engine` and `intent` are disjoint fields, so the ids drain straight into the engine.
        let failures = self
            .engine
            .evict_cache(self.intent.evict_cache.drain(..))
            .await;
        if failures.is_empty() {
            return;
        }

        self.reports
            .publish(WorkerReport::Error(WorkerError::CacheEviction { failures }));
    }

    /// Persist resident disk-backed values: the nodes this batch named, then, if it also asked
    /// for the sweep, every installed node.
    ///
    /// After the eviction, so a node this batch names in both ends up with nothing on disk. The
    /// named half runs first: the sweep would otherwise reach those nodes as ordinary members and
    /// not report a type that will never persist.
    async fn flush_cache(&mut self) {
        if !self.intent.flush_cache.is_empty() {
            let report = self
                .engine
                .flush_cache(self.intent.flush_cache.drain(..))
                .await;
            self.report_flush(report, FlushReporting::Requested);
        }
        if mem::take(&mut self.intent.flush_all_caches) {
            let report = self.engine.flush_all_caches().await;
            self.report_flush(report, FlushReporting::Sweep);
        }
    }

    /// Raise whatever a flush left unwritten, under `reporting`'s policy; a flush that wrote
    /// everything reports nothing.
    fn report_flush(&self, report: CacheFlushReport, reporting: FlushReporting) {
        let CacheFlushReport {
            failures,
            unsupported,
        } = report;
        // The policy is the worker's: nothing correlates a report back to the host's command.
        let unsupported = match reporting {
            FlushReporting::Requested => unsupported,
            FlushReporting::Sweep => Vec::new(),
        };
        if failures.is_empty() && unsupported.is_empty() {
            return;
        }
        self.reports
            .publish(WorkerReport::Error(WorkerError::CacheFlush {
                failures,
                unsupported,
            }));
    }

    /// Run the batch's seeds, which stay in the batch so their buffers come back to the mailbox;
    /// a `Rebuild` starts the loop over the events the run armed.
    async fn execute(&mut self, transition: EventLoopTransition) {
        if self.mailbox.shutting_down() {
            return;
        }
        let activity = self.executing_activity();
        self.reports.publish(WorkerReport::Activity(activity));
        let mut reporter = WorkerRunReporter {
            reports: &self.reports,
        };
        let result = self
            .engine
            .execute(
                ExecuteRequest {
                    seeds: &self.intent.seeds,
                    reporter: &mut reporter,
                    cancel: self.mailbox.cancel_token(),
                },
                &mut self.outcome,
            )
            .await;

        match result {
            Ok(()) => {
                if transition == EventLoopTransition::Rebuild && !self.mailbox.shutting_down() {
                    assert!(self.event_loop.is_none());
                    let armed = mem::take(&mut self.outcome.armed_events);
                    if !armed.is_empty() {
                        self.event_loop =
                            Some(ActiveEventLoop::start(armed, &self.mailbox, &self.epoch));
                    }
                }
                let activity = self.resting_activity();
                self.reports.publish(WorkerReport::Completed(
                    self.summary.publish(activity, &mut self.outcome),
                ));
            }
            Err(error) => {
                let activity = self.resting_activity();
                self.reports.publish(WorkerReport::Activity(activity));
                self.reports
                    .publish(WorkerReport::Error(WorkerError::Exec { error }));
            }
        }
    }

    /// Stop the loop and report `Idle`, as on worker shutdown; intent application stops
    /// [quietly](IdleReport::Quiet) instead.
    async fn stop_event_loop(&mut self) {
        self.finish_event_loop(IdleReport::Report).await;
    }

    /// Stop the loop, then forget what its tasks fired after this batch was taken: none fires
    /// again once they are joined.
    async fn finish_event_loop(&mut self, idle: IdleReport) {
        let Some(mut active) = self.event_loop.take() else {
            return;
        };
        active.stop().await;
        self.mailbox.void_events();
        if idle == IdleReport::Report {
            self.reports
                .publish(WorkerReport::Activity(WorkerActivity::Idle));
        }
    }

    const fn executing_activity(&self) -> WorkerActivity {
        match &self.event_loop {
            Some(_) => WorkerActivity::ExecutingEventLoop,
            None => WorkerActivity::Executing,
        }
    }

    const fn resting_activity(&self) -> WorkerActivity {
        match &self.event_loop {
            Some(_) => WorkerActivity::EventLoop,
            None => WorkerActivity::Idle,
        }
    }
}

/// Publishes a run's live feedback to the host as the run loop produces it, by value, so the
/// executor can hand each event straight over instead of queueing it for a relay to drain.
#[derive(Debug)]
struct WorkerRunReporter<'a> {
    reports: &'a ReportSink,
}

impl RunReporter for WorkerRunReporter<'_> {
    fn progress(&mut self, program_id: ProgramId, phase: RunPhase) {
        self.reports
            .publish(WorkerReport::Progress { program_id, phase });
    }
}

#[cfg(test)]
mod tests;
