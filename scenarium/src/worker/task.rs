use std::fmt;
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

use ::common::CancelToken;

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

        if let Some(cache) = self.intent.disk_store.take() {
            self.engine.set_disk_store(cache);
            self.engine.store_resident_caches().await;
        }

        match self.intent.graph_state.take() {
            Some(GraphOp::Clear) => {
                self.engine.clear();
                (self.callback)(WorkerReport::Cleared);
            }
            Some(GraphOp::Replace(compiled)) => {
                tracing::info!("Graph updated");
                self.engine.install(Arc::clone(&compiled));
                (self.callback)(WorkerReport::Installed(compiled));
            }
            None => {}
        }

        self.evict_cache().await;

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

        let node_ids = self.intent.evict_cache.drain(..).collect::<Vec<_>>();
        let failures = self.engine.evict_cache(&node_ids).await;
        if failures.is_empty() {
            return;
        }

        let details = failures
            .iter()
            .map(|failure| format!("{:?}: {}", failure.node_id, failure.message))
            .collect::<Vec<_>>()
            .join("; ");
        (self.callback)(WorkerReport::Error(WorkerError::CacheEviction {
            failure_count: failures.len(),
            details,
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
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use ::common::CancelToken;

    use crate::execution::report::NodeExecutionStatus;
    use crate::execution::report::{RunPhase, RunProgress, RunReporter};
    use crate::execution::seeds::RunSeeds;
    use crate::graph::identity::NodeId;
    use crate::worker::batch::{BatchIntent, GraphOp, LoopCommand};
    use crate::worker::protocol::{WorkerMessage, WorkerReport};
    use crate::worker::status::{WorkerActivity, WorkerStatusKind, WorkerStatusPublisher};
    use crate::worker::task::{EventLoopTransition, PendingRun, WorkerRunReporter, WorkerTask};

    #[tokio::test]
    async fn next_intent_receives_many_messages_into_a_reusable_buffer() {
        let (tx, rx) = mpsc::unbounded_channel();
        let node_id = NodeId::unique();
        tx.send(WorkerMessage::Clear).unwrap();
        tx.send(WorkerMessage::Run {
            seeds: RunSeeds::nodes(vec![node_id]),
        })
        .unwrap();
        let shutdown = CancellationToken::new();
        let mut task = WorkerTask::new(
            rx,
            |_: WorkerReport| {},
            CancelToken::new(),
            shutdown.clone(),
        );

        {
            let intent = task.next_intent().await.unwrap();
            assert!(matches!(intent.graph_state, Some(GraphOp::Clear)));
            assert_eq!(intent.seeds.node_ids, [node_id]);
        }
        assert!(task.messages.is_empty());
        let capacity = task.messages.capacity();
        assert!(capacity >= 2);

        tx.send(WorkerMessage::StopEventLoop).unwrap();
        let intent = task.next_intent().await.unwrap();
        assert!(matches!(intent.loop_request, Some(LoopCommand::Stop)));
        assert_eq!(task.messages.capacity(), capacity);

        tx.send(WorkerMessage::Clear).unwrap();
        shutdown.cancel();
        assert!(task.next_intent().await.is_none());
    }

    #[test]
    fn event_loop_transition_covers_commands_and_graph_replacement() {
        let cases = [
            (BatchIntent::default(), false, EventLoopTransition::Preserve),
            (BatchIntent::default(), true, EventLoopTransition::Preserve),
            (
                BatchIntent {
                    loop_request: Some(LoopCommand::Start),
                    ..BatchIntent::default()
                },
                false,
                EventLoopTransition::Rebuild,
            ),
            (
                BatchIntent {
                    loop_request: Some(LoopCommand::Start),
                    ..BatchIntent::default()
                },
                true,
                EventLoopTransition::Rebuild,
            ),
            (
                BatchIntent {
                    loop_request: Some(LoopCommand::Stop),
                    ..BatchIntent::default()
                },
                false,
                EventLoopTransition::Stop,
            ),
            (
                BatchIntent {
                    loop_request: Some(LoopCommand::Stop),
                    ..BatchIntent::default()
                },
                true,
                EventLoopTransition::Stop,
            ),
            (
                BatchIntent {
                    graph_state: Some(GraphOp::Clear),
                    ..BatchIntent::default()
                },
                false,
                EventLoopTransition::Preserve,
            ),
            (
                BatchIntent {
                    graph_state: Some(GraphOp::Clear),
                    ..BatchIntent::default()
                },
                true,
                EventLoopTransition::Rebuild,
            ),
            (
                BatchIntent {
                    graph_state: Some(GraphOp::Clear),
                    loop_request: Some(LoopCommand::Stop),
                    ..BatchIntent::default()
                },
                true,
                EventLoopTransition::Stop,
            ),
        ];

        for (intent, active, expected) in cases {
            assert_eq!(EventLoopTransition::for_intent(&intent, active), expected);
        }
    }

    #[test]
    fn pending_run_couples_event_source_initialization_to_loop_rebuild() {
        let mut empty = BatchIntent::default();
        assert!(PendingRun::take(&mut empty, EventLoopTransition::Preserve).is_none());

        let mut rebuild = BatchIntent::default();
        let run = PendingRun::take(&mut rebuild, EventLoopTransition::Rebuild).unwrap();
        assert!(run.start_event_loop);
        assert!(run.seeds.event_sources);
        assert!(!run.seeds.sinks);
        assert!(run.seeds.events.is_empty());
        assert!(run.seeds.node_ids.is_empty());

        let node_id = NodeId::unique();
        let mut explicit = BatchIntent::default();
        explicit.reset(
            [WorkerMessage::Run {
                seeds: RunSeeds::nodes(vec![node_id]),
            }],
            [],
        );
        let run = PendingRun::take(&mut explicit, EventLoopTransition::Preserve).unwrap();
        assert!(!run.start_event_loop);
        assert!(!run.seeds.event_sources);
        assert_eq!(run.seeds.node_ids, [node_id]);
    }

    /// Each reported event publishes its own snapshot the moment it happens, and a snapshot
    /// the host has not drained yet is never mutated by the next one.
    #[test]
    fn worker_reporter_publishes_each_event_and_preserves_published_snapshots() {
        let first_node = NodeId::unique();
        let second_node = NodeId::unique();
        let mut status = WorkerStatusPublisher::default();
        drop(status.activity(WorkerActivity::Executing));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let callback = |report| tx.send(report).unwrap();
        let mut reporter = WorkerRunReporter {
            status: &mut status,
            callback: &callback,
        };

        reporter.progress(RunProgress {
            node_id: first_node,
            phase: RunPhase::Started { at: Instant::now() },
        });
        reporter.progress(RunProgress {
            node_id: second_node,
            phase: RunPhase::Finished { elapsed_secs: 0.25 },
        });

        let WorkerReport::Status(started) = rx.try_recv().unwrap() else {
            panic!("progress must produce a status patch");
        };
        let WorkerReport::Status(finished) = rx.try_recv().unwrap() else {
            panic!("progress must produce a status patch");
        };
        assert!(rx.try_recv().is_err());
        assert_eq!(started.kind, WorkerStatusKind::Patch);
        assert_eq!(started.activity, WorkerActivity::Executing);
        assert_eq!(started.nodes.len(), 1);
        assert_eq!(started.nodes[0].node_id, first_node);
        assert!(matches!(
            started.nodes[0].status,
            Some(NodeExecutionStatus::Running { .. })
        ));
        assert_eq!(finished.nodes.len(), 1);
        assert_eq!(finished.nodes[0].node_id, second_node);
        assert!(matches!(
            finished.nodes[0].status,
            Some(NodeExecutionStatus::Executed { elapsed_secs: 0.25 })
        ));
        // The second patch could not reuse the first's still-queued allocation.
        assert!(!Arc::ptr_eq(&started, &finished));

        // Publishing over a still-queued snapshot allocates fresh rather than deep-cloning
        // vectors it immediately clears — a clone would carry the previous capacity over.
        let idle = status.activity(WorkerActivity::Idle);
        assert!(idle.nodes.is_empty());
        assert_eq!(idle.nodes.capacity(), 0);
        assert_eq!(started.nodes.len(), 1, "a published snapshot is immutable");

        drop((started, finished));
        let allocation = Arc::as_ptr(&idle);
        drop(idle);
        let executing = status.activity(WorkerActivity::Executing);
        assert_eq!(
            Arc::as_ptr(&executing),
            allocation,
            "a drained snapshot's allocation is recycled"
        );
        assert!(rx.try_recv().is_err());
    }
}
