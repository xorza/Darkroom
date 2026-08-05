use std::fmt;
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use ::common::CancelToken;

use crate::execution::cache::runtime::error::CacheFlushReport;
use crate::execution::engine::ExecutionEngine;
use crate::execution::error::Error;
use crate::execution::report::ExecutionOutcome;
use crate::execution::report::{RunProgress, RunReporter};
use crate::execution::seeds::RunSeeds;
use crate::graph::identity::EventPort;
use crate::worker::batch::{BatchIntent, GraphOp, LoopCommand};
use crate::worker::error::WorkerError;
use crate::worker::event_loop::{
    ActiveEventLoop, EVENT_LOOP_BACKPRESSURE, EventLoopWake, LambdaPanic,
};
use crate::worker::pause_gate::PauseGate;
use crate::worker::protocol::{WorkerMessage, WorkerReport};
use crate::worker::status::{WorkerActivity, WorkerStatusPublisher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventLoopTransition {
    Preserve,
    Stop,
    Rebuild,
}

impl EventLoopTransition {
    fn for_intent(intent: &BatchIntent, event_loop_active: bool) -> Self {
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
    /// A flush the host asked for, by node. Everything it did not write is the
    /// answer to that request — a type that will never persist included, since
    /// the node goes on showing itself disk-backed with nothing behind it.
    Requested,
    /// The sweep a newly attached store owes every already-resident value.
    /// Nobody named these nodes, so an unpersistable type among them is a
    /// standing fact about the library rather than news; only a write that
    /// broke is worth raising.
    Sweep,
}

#[derive(Debug)]
struct PendingRun {
    seeds: RunSeeds,
    start_event_loop: bool,
}

impl PendingRun {
    fn take(intent: &mut BatchIntent, transition: EventLoopTransition) -> Option<Self> {
        let start_event_loop = matches!(transition, EventLoopTransition::Rebuild);
        if intent.seeds.is_empty() && !start_event_loop {
            return None;
        }

        // Moved out rather than copied field by field: the batch's seeds *are*
        // the run's, and taking them leaves the intent empty for whatever the
        // rest of this batch still does.
        let mut seeds = std::mem::take(&mut intent.seeds);
        // Rebuilding the loop means re-initializing every event source, so the
        // bootstrap run demands them whether or not a message asked.
        seeds.event_sources |= start_event_loop;
        Some(Self {
            seeds,
            start_event_loop,
        })
    }
}

#[derive(Debug)]
enum WorkerWake {
    Ready,
    Stopped,
    EventLoopPanicked(LambdaPanic),
}

#[derive(Debug)]
pub(crate) struct WorkerTask<ExecutionCallback> {
    message_rx: UnboundedReceiver<WorkerMessage>,
    callback: ExecutionCallback,
    run_cancel: CancelToken,
    shutdown: CancellationToken,
    engine: ExecutionEngine,
    status: WorkerStatusPublisher,
    outcome: ExecutionOutcome,
    intent: BatchIntent,
    messages: Vec<WorkerMessage>,
    event_buffer: Vec<EventPort>,
    event_loop: Option<ActiveEventLoop>,
    event_loop_pause_gate: PauseGate,
}

impl<ExecutionCallback> WorkerTask<ExecutionCallback>
where
    ExecutionCallback: Fn(WorkerReport) + Send + Sync + 'static,
{
    pub(crate) fn new(
        message_rx: UnboundedReceiver<WorkerMessage>,
        callback: ExecutionCallback,
        run_cancel: CancelToken,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            message_rx,
            callback,
            run_cancel,
            shutdown,
            engine: ExecutionEngine::default(),
            status: WorkerStatusPublisher::default(),
            outcome: ExecutionOutcome::default(),
            intent: BatchIntent::default(),
            messages: Vec::new(),
            event_buffer: Vec::with_capacity(EVENT_LOOP_BACKPRESSURE),
            event_loop: None,
            event_loop_pause_gate: PauseGate::default(),
        }
    }

    pub(crate) async fn run(mut self) {
        while self.next_intent().await.is_some() {
            self.apply_intent().await;
            tokio::task::yield_now().await;
        }
        self.stop_event_loop().await;
    }

    async fn next_intent(&mut self) -> Option<&BatchIntent> {
        loop {
            self.messages.clear();
            self.event_buffer.clear();
            let wake = tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => WorkerWake::Stopped,
                count = self.message_rx.recv_many(&mut self.messages, usize::MAX) => match count {
                    0 => WorkerWake::Stopped,
                    _ => WorkerWake::Ready,
                },
                wake = async {
                    self.event_loop.as_mut().unwrap().recv(&mut self.event_buffer).await
                }, if self.event_loop.is_some() => match wake {
                    EventLoopWake::Events => WorkerWake::Ready,
                    EventLoopWake::TaskPanicked(panic) => WorkerWake::EventLoopPanicked(panic),
                },
            };

            match wake {
                WorkerWake::Ready => {}
                WorkerWake::Stopped => return None,
                WorkerWake::EventLoopPanicked(panic) => {
                    self.fail_event_loop(panic).await;
                    continue;
                }
            }
            self.intent
                .reset(self.messages.drain(..), self.event_buffer.drain(..));
            // The cancellation commit boundary: a cancel raised from here on
            // targets this batch's (possibly imminent) run; one raised while
            // nothing was committed clears now instead of leaking into it.
            self.run_cancel.reset();
            return Some(&self.intent);
        }
    }

    async fn apply_intent(&mut self) {
        let transition = EventLoopTransition::for_intent(&self.intent, self.event_loop.is_some());
        let stopped_loop =
            !matches!(transition, EventLoopTransition::Preserve) && self.event_loop.is_some();
        if stopped_loop {
            self.finish_event_loop(None, false).await;
        }

        // The graph op leads: it establishes the program every stage below is
        // about. Applied after the store, the store's sweep wrote the outgoing
        // document's values into the incoming document's root — see
        // [`BatchIntent`].
        match self.intent.graph_state.take() {
            Some(GraphOp::Clear) => {
                self.engine.clear();
                (self.callback)(WorkerReport::Cleared);
            }
            Some(GraphOp::Replace(compiled)) => {
                tracing::info!("Graph updated");
                self.engine.install(Arc::clone(&compiled));
                // After the install, not before: reconciling onto the new
                // program is what released the slots of the nodes it dropped.
                let cache_ram = self.engine.resident_cache_ram();
                (self.callback)(WorkerReport::Installed {
                    compiled,
                    cache_ram,
                });
            }
            None => {}
        }

        if let Some(cache) = self.intent.disk_store.take() {
            self.engine.set_disk_store(cache);
        }

        self.evict_cache().await;
        self.flush_cache().await;

        let mut ran = false;
        if let Some(run) = PendingRun::take(&mut self.intent, transition)
            && !self.engine.is_empty()
        {
            self.execute(run).await;
            ran = true;
        }
        // A stopped loop with no follow-up run really is idle; a rebuild's
        // stop is not — its `execute` reports `Executing` directly, without
        // a transient `Idle` flashing in between.
        if stopped_loop && !ran {
            (self.callback)(WorkerReport::Status(
                self.status.activity(WorkerActivity::Idle),
            ));
        }

        for reply in self.intent.syncs.drain(..) {
            let _ = reply.send(());
        }
    }

    async fn evict_cache(&mut self) {
        if self.intent.evict_cache.is_empty() {
            return;
        }

        // Drained straight into the engine, which consumes the ids exactly once:
        // `engine` and `intent` are disjoint fields, so nothing has to be
        // collected out of the batch first.
        let failures = self
            .engine
            .evict_cache(self.intent.evict_cache.drain(..))
            .await;
        if failures.is_empty() {
            return;
        }

        (self.callback)(WorkerReport::Error(WorkerError::CacheEviction { failures }));
    }

    /// Persist resident disk-backed values: the nodes this batch named, then —
    /// if it also asked for the sweep — every installed node.
    ///
    /// After the eviction rather than before it, so a node this batch names in
    /// both ends up with what the eviction asked for — nothing on disk — rather
    /// than with a blob the flush re-wrote behind it.
    ///
    /// Both halves in one stage, and in this order, because they differ only in
    /// what they report: a node the host named is owed an answer about a type
    /// that will never persist, and the sweep is not. Running the named half
    /// first is what keeps that answer — the sweep would otherwise reach those
    /// nodes as ordinary members and report nothing about them.
    async fn flush_cache(&mut self) {
        if !self.intent.flush_cache.is_empty() {
            let report = self
                .engine
                .flush_cache(self.intent.flush_cache.drain(..))
                .await;
            self.report_flush(report, FlushReporting::Requested);
        }
        if std::mem::take(&mut self.intent.flush_all_caches) {
            let report = self.engine.flush_all_caches().await;
            self.report_flush(report, FlushReporting::Sweep);
        }
    }

    /// Raise whatever a flush left unwritten, under `reporting`'s policy.
    ///
    /// A flush that wrote everything reports nothing — silence here means the
    /// blobs are there, which is the one thing the host could not previously
    /// tell apart from a store that failed on every node.
    fn report_flush(&self, report: CacheFlushReport, reporting: FlushReporting) {
        let CacheFlushReport {
            failures,
            unsupported,
        } = report;
        // The policy is the worker's because only it knows whether a node was
        // named: the host sent the command, but nothing correlates a report
        // back to one.
        let unsupported = match reporting {
            FlushReporting::Requested => unsupported,
            FlushReporting::Sweep => Vec::new(),
        };
        if failures.is_empty() && unsupported.is_empty() {
            return;
        }
        (self.callback)(WorkerReport::Error(WorkerError::CacheFlush {
            failures,
            unsupported,
        }));
    }

    async fn execute(&mut self, run: PendingRun) {
        if self.shutdown.is_cancelled() {
            return;
        }
        let activity = self.executing_activity();
        (self.callback)(WorkerReport::Status(self.status.activity(activity)));
        let _pause_guard = self.event_loop_pause_gate.close();
        let mut reporter = WorkerRunReporter {
            status: &mut self.status,
            callback: &self.callback,
        };
        let result = self
            .engine
            .execute(
                run.seeds,
                &mut reporter,
                self.run_cancel.clone(),
                &mut self.outcome,
            )
            .await;

        match result {
            Ok(()) => {
                if run.start_event_loop && !self.shutdown.is_cancelled() {
                    assert!(self.event_loop.is_none());
                    let triggers = std::mem::take(&mut self.outcome.event_triggers);
                    if !triggers.is_empty() {
                        self.event_loop = Some(
                            ActiveEventLoop::start(triggers, self.event_loop_pause_gate.clone())
                                .await,
                        );
                        tracing::info!("Event loop started");
                    }
                }
                let activity = self.resting_activity();
                (self.callback)(WorkerReport::Status(
                    self.status.completed(activity, &mut self.outcome),
                ));
            }
            Err(error) => {
                let activity = self.resting_activity();
                (self.callback)(WorkerReport::Status(self.status.activity(activity)));
                (self.callback)(WorkerReport::Error(WorkerError::Execution { error }));
            }
        }
    }

    /// Stop the loop and report `Idle` — the terminal form (worker
    /// shutdown). Intent application stops quietly instead
    /// (`finish_event_loop(None, false)`): a rebuild's `execute` reports
    /// `Executing` directly, without a transient `Idle` flashing between.
    async fn stop_event_loop(&mut self) {
        self.finish_event_loop(None, true).await;
    }

    async fn fail_event_loop(&mut self, panic: LambdaPanic) {
        self.finish_event_loop(Some(panic), true).await;
    }

    async fn finish_event_loop(&mut self, leading_panic: Option<LambdaPanic>, report_idle: bool) {
        let Some(mut active) = self.event_loop.take() else {
            assert!(
                leading_panic.is_none(),
                "event task panic received without an active event loop"
            );
            return;
        };

        let mut panics = active.stop().await;
        if let Some(panic) = leading_panic {
            panics.insert(0, panic);
        }
        tracing::info!("Event loop stopped");
        if report_idle {
            (self.callback)(WorkerReport::Status(
                self.status.activity(WorkerActivity::Idle),
            ));
        }
        for panic in panics {
            (self.callback)(WorkerReport::Error(WorkerError::Execution {
                error: Error::EventLambdaPanic {
                    node_id: panic.node_id,
                    message: panic.message,
                },
            }));
        }
    }

    fn executing_activity(&self) -> WorkerActivity {
        match &self.event_loop {
            Some(_) => WorkerActivity::ExecutingEventLoop,
            None => WorkerActivity::Executing,
        }
    }

    fn resting_activity(&self) -> WorkerActivity {
        match &self.event_loop {
            Some(_) => WorkerActivity::EventLoop,
            None => WorkerActivity::Idle,
        }
    }
}

/// Publishes a run's live feedback to the host as the run loop produces it. Owns the
/// borrows a report needs — the status publisher's retained allocation and the host
/// callback — so the executor can hand each event straight over instead of queueing it for
/// a relay to drain.
struct WorkerRunReporter<'a, C> {
    status: &'a mut WorkerStatusPublisher,
    callback: &'a C,
}

impl<C> fmt::Debug for WorkerRunReporter<'_, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerRunReporter").finish_non_exhaustive()
    }
}

impl<C> RunReporter for WorkerRunReporter<'_, C>
where
    C: Fn(WorkerReport) + Sync,
{
    fn progress(&mut self, progress: RunProgress) {
        let mut patch = self.status.patch();
        patch.push(progress);
        (self.callback)(WorkerReport::Status(patch.finish()));
    }
}

#[cfg(test)]
mod tests;
