use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio_util::sync::CancellationToken;

use common::CancelToken;

use crate::execution::engine::ExecutionEngine;
use crate::execution::error::{Error, Result};
use crate::execution::identity::ExecutionEventPort;
use crate::execution::outcome::ExecutionOutcome;
use crate::execution::report::RunEvent;
use crate::execution::seeds::RunSeeds;
use crate::worker::batch::{BatchIntent, GraphOp, LoopCommand};
use crate::worker::event_loop::{
    ActiveEventLoop, EVENT_LOOP_BACKPRESSURE, EventLoopWake, LambdaPanic,
};
use crate::worker::pause_gate::PauseGate;
use crate::worker::protocol::{WorkerError, WorkerMessage, WorkerReport};
use crate::worker::status::{WorkerActivity, WorkerStatusPublisher};

const RUN_EVENT_BATCH_SIZE: usize = 64;

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
        let has_run = intent.execute_sinks
            || intent.execute_event_sources
            || !intent.execute_nodes.is_empty()
            || !intent.events.is_empty()
            || start_event_loop;
        if !has_run {
            return None;
        }

        Some(Self {
            seeds: RunSeeds {
                sinks: intent.execute_sinks,
                event_sources: start_event_loop || intent.execute_event_sources,
                events: intent.events.drain(..).collect(),
                nodes: intent.execute_nodes.drain(..).collect(),
            },
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
    event_buffer: Vec<ExecutionEventPort>,
    run_events: Vec<RunEvent>,
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
            run_events: Vec::with_capacity(RUN_EVENT_BATCH_SIZE),
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
            return Some(&self.intent);
        }
    }

    async fn apply_intent(&mut self) {
        let transition = EventLoopTransition::for_intent(&self.intent, self.event_loop.is_some());
        if !matches!(transition, EventLoopTransition::Preserve) {
            self.stop_event_loop().await;
        }

        if let Some(cache) = self.intent.disk_store.take() {
            self.engine.cache.disk_store = cache;
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

        if let Some(run) = PendingRun::take(&mut self.intent, transition)
            && !self.engine.is_empty()
        {
            self.execute(run).await;
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
            .map(|failure| format!("{:?}: {}", failure.e_node_id, failure.message))
            .collect::<Vec<_>>()
            .join("; ");
        (self.callback)(WorkerReport::Error(WorkerError::CacheEviction {
            failure_count: failures.len(),
            details,
        }));
    }

    async fn execute(&mut self, run: PendingRun) {
        self.run_cancel.reset();
        if self.shutdown.is_cancelled() {
            return;
        }
        let activity = self.executing_activity();
        (self.callback)(WorkerReport::Status(self.status.activity(activity)));
        let _pause_guard = self.event_loop_pause_gate.close();
        let result = run_and_forward(
            &mut self.engine,
            &mut self.outcome,
            &mut self.run_events,
            &mut self.status,
            run.seeds,
            self.run_cancel.clone(),
            &self.callback,
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

    async fn stop_event_loop(&mut self) {
        self.finish_event_loop(None).await;
    }

    async fn fail_event_loop(&mut self, panic: LambdaPanic) {
        self.finish_event_loop(Some(panic)).await;
    }

    async fn finish_event_loop(&mut self, leading_panic: Option<LambdaPanic>) {
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
        (self.callback)(WorkerReport::Status(
            self.status.activity(WorkerActivity::Idle),
        ));
        for panic in panics {
            (self.callback)(WorkerReport::Error(WorkerError::Execution {
                error: Error::EventLambdaPanic {
                    e_node_id: panic.e_node_id,
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

async fn run_and_forward<C>(
    engine: &mut ExecutionEngine,
    outcome: &mut ExecutionOutcome,
    events: &mut Vec<RunEvent>,
    status: &mut WorkerStatusPublisher,
    seeds: RunSeeds,
    cancel: CancelToken,
    callback: &C,
) -> Result<()>
where
    C: Fn(WorkerReport) + Sync,
{
    events.clear();
    let (event_tx, mut event_rx) = unbounded_channel::<RunEvent>();
    let result = {
        let run = engine.execute(seeds, Some(&event_tx), cancel, outcome);
        tokio::pin!(run);
        loop {
            tokio::select! {
                biased;
                result = &mut run => break result,
                _ = event_rx.recv_many(events, RUN_EVENT_BATCH_SIZE) => {
                    forward_run_events(events, status, callback);
                }
            }
        }
    };
    drop(event_tx);
    event_rx.recv_many(events, usize::MAX).await;
    forward_run_events(events, status, callback);
    result
}

fn forward_run_events<C>(
    events: &mut Vec<RunEvent>,
    status: &mut WorkerStatusPublisher,
    callback: &C,
) where
    C: Fn(WorkerReport),
{
    let mut events = events.drain(..).peekable();
    while let Some(event) = events.next() {
        match event {
            RunEvent::Progress(progress) => {
                let mut patch = status.patch();
                patch.push(progress);
                while let Some(RunEvent::Progress(progress)) =
                    events.next_if(|event| matches!(event, RunEvent::Progress(_)))
                {
                    patch.push(progress);
                }
                callback(WorkerReport::Status(patch.finish()));
            }
            RunEvent::PinnedOutputs(outputs) => callback(WorkerReport::PinnedOutputs(outputs)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use common::CancelToken;

    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::report::{RunEvent, RunPhase, RunProgress};
    use crate::execution::seeds::RunSeeds;
    use crate::worker::batch::{BatchIntent, GraphOp, LoopCommand};
    use crate::worker::protocol::{WorkerMessage, WorkerReport};
    use crate::worker::status::{
        NodeExecutionStatus, WorkerActivity, WorkerStatusKind, WorkerStatusPublisher,
    };
    use crate::worker::task::{EventLoopTransition, PendingRun, WorkerTask, forward_run_events};

    #[tokio::test]
    async fn next_intent_receives_many_messages_into_a_reusable_buffer() {
        let (tx, rx) = mpsc::unbounded_channel();
        let e_node_id = ExecutionNodeId::unique();
        tx.send(WorkerMessage::Clear).unwrap();
        tx.send(WorkerMessage::Run {
            seeds: RunSeeds::nodes(vec![e_node_id]),
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
            assert_eq!(
                intent.execute_nodes.iter().copied().collect::<Vec<_>>(),
                [e_node_id]
            );
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
        assert!(run.seeds.nodes.is_empty());

        let e_node_id = ExecutionNodeId::unique();
        let mut explicit = BatchIntent::default();
        explicit.reset(
            [WorkerMessage::Run {
                seeds: RunSeeds::nodes(vec![e_node_id]),
            }],
            [],
        );
        let run = PendingRun::take(&mut explicit, EventLoopTransition::Preserve).unwrap();
        assert!(!run.start_event_loop);
        assert!(!run.seeds.event_sources);
        assert_eq!(run.seeds.nodes, [e_node_id]);
    }

    #[test]
    fn worker_status_batches_nodes_and_preserves_published_snapshots() {
        let first_node = ExecutionNodeId::unique();
        let second_node = ExecutionNodeId::unique();
        let mut events = vec![
            RunEvent::Progress(RunProgress {
                e_node_id: first_node,
                phase: RunPhase::Started { at: Instant::now() },
            }),
            RunEvent::Progress(RunProgress {
                e_node_id: second_node,
                phase: RunPhase::Finished { elapsed_secs: 0.25 },
            }),
        ];
        let mut status = WorkerStatusPublisher::default();
        drop(status.activity(WorkerActivity::Executing));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let callback = |report| tx.send(report).unwrap();

        forward_run_events(&mut events, &mut status, &callback);
        let WorkerReport::Status(patch) = rx.try_recv().unwrap() else {
            panic!("progress must produce a status patch");
        };
        assert_eq!(patch.kind, WorkerStatusKind::Patch);
        assert_eq!(patch.nodes.len(), 2);
        assert_eq!(patch.nodes[0].e_node_id, first_node);
        assert!(matches!(
            patch.nodes[0].status,
            Some(NodeExecutionStatus::Running { .. })
        ));
        assert_eq!(patch.nodes[1].e_node_id, second_node);
        assert!(matches!(
            patch.nodes[1].status,
            Some(NodeExecutionStatus::Executed { elapsed_secs: 0.25 })
        ));
        let idle = status.activity(WorkerActivity::Idle);
        assert!(!Arc::ptr_eq(&patch, &idle));
        assert_eq!(patch.activity, WorkerActivity::Executing);
        assert_eq!(patch.nodes.len(), 2);
        assert_eq!(idle.activity, WorkerActivity::Idle);
        assert!(idle.nodes.is_empty());

        drop(patch);
        let allocation = Arc::as_ptr(&idle);
        drop(idle);
        let executing = status.activity(WorkerActivity::Executing);
        assert_eq!(Arc::as_ptr(&executing), allocation);
        assert!(rx.try_recv().is_err());
    }
}
