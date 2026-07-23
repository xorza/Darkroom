use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use common::CancelToken;

use crate::execution::event::EventTrigger;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::outcome::ExecutionOutcome;
use crate::execution::report::RunEvent;
use crate::execution::{Error, ExecutionEngine, Result, RunSeeds};
use crate::worker::batch::{BatchIntent, GraphOp, LoopCommand, scan};
use crate::worker::event_loop::{
    ActiveEventLoop, EVENT_LOOP_BACKPRESSURE, StopOutcome, stop_event_loop,
};
use crate::worker::pause_gate::PauseGate;
use crate::worker::protocol::{WorkerError, WorkerMessage, WorkerReport};
use crate::worker::status::{WorkerActivity, WorkerStatusPublisher};

const RUN_EVENT_BATCH_SIZE: usize = 64;

#[derive(Debug)]
struct WorkerTask {
    engine: ExecutionEngine,
    event_loop: Option<ActiveEventLoop>,
    event_loop_pause_gate: PauseGate,
    status: WorkerStatusPublisher,
    cancel: CancelToken,
    outcome: ExecutionOutcome,
    event_loop_events: Vec<ExecutionEventPort>,
    run_events: Vec<RunEvent>,
}

impl WorkerTask {
    fn new(cancel: CancelToken) -> Self {
        Self {
            engine: ExecutionEngine::default(),
            event_loop: None,
            event_loop_pause_gate: PauseGate::default(),
            status: WorkerStatusPublisher::default(),
            cancel,
            outcome: ExecutionOutcome::default(),
            event_loop_events: Vec::with_capacity(EVENT_LOOP_BACKPRESSURE),
            run_events: Vec::with_capacity(RUN_EVENT_BATCH_SIZE),
        }
    }

    async fn process<C>(mut self, mut messages: UnboundedReceiver<Vec<WorkerMessage>>, callback: &C)
    where
        C: Fn(WorkerReport) + Sync,
    {
        while let Some(intent) = self.next_intent(&mut messages, callback).await {
            if !self.process_intent(intent, callback).await {
                return;
            }
            tokio::task::yield_now().await;
        }
    }

    async fn next_intent<C>(
        &mut self,
        messages: &mut UnboundedReceiver<Vec<WorkerMessage>>,
        callback: &C,
    ) -> Option<BatchIntent>
    where
        C: Fn(WorkerReport),
    {
        loop {
            self.event_loop_events.clear();
            let mut intent = tokio::select! {
                biased;
                batch = messages.recv() => scan(batch?),
                count = async {
                    self.event_loop.as_mut().unwrap().events
                        .recv_many(&mut self.event_loop_events, EVENT_LOOP_BACKPRESSURE).await
                }, if self.event_loop.is_some() => {
                    if count == 0 {
                        self.stop_event_loop(callback).await;
                        continue;
                    }
                    BatchIntent::default()
                }
            };
            intent.events.extend(self.event_loop_events.drain(..));
            return Some(intent);
        }
    }

    async fn process_intent<C>(&mut self, mut intent: BatchIntent, callback: &C) -> bool
    where
        C: Fn(WorkerReport) + Sync,
    {
        let restart_event_loop = self.stop_event_loop_for(&intent, callback).await;
        if intent.exit {
            return false;
        }

        self.apply_disk_store(&mut intent).await;
        self.apply_graph_state(&mut intent, callback);
        self.evict_cache(&mut intent, callback).await;

        if restart_event_loop && intent.loop_command.is_none() {
            intent.loop_command = Some(LoopCommand::Start);
        }
        self.execute_intent(&mut intent, callback).await;

        for reply in intent.syncs {
            let _ = reply.send(());
        }
        true
    }

    async fn stop_event_loop_for<C>(&mut self, intent: &BatchIntent, callback: &C) -> bool
    where
        C: Fn(WorkerReport),
    {
        let needs_stop =
            intent.graph_state.is_some() || intent.loop_command.is_some() || intent.exit;
        if needs_stop {
            self.stop_event_loop(callback).await
        } else {
            false
        }
    }

    async fn stop_event_loop<C>(&mut self, callback: &C) -> bool
    where
        C: Fn(WorkerReport),
    {
        let outcome = stop_event_loop(&mut self.event_loop).await;
        let was_running = outcome.was_running;
        self.report_event_loop_stop(outcome, callback);
        was_running
    }

    fn report_event_loop_stop<C>(&mut self, outcome: StopOutcome, callback: &C)
    where
        C: Fn(WorkerReport),
    {
        if outcome.was_running {
            callback(WorkerReport::Status(
                self.status.activity(WorkerActivity::Idle),
            ));
        }
        for panic in outcome.panics {
            callback(WorkerReport::Error(WorkerError::Execution {
                error: Error::EventLambdaPanic {
                    e_node_id: panic.e_node_id,
                    message: panic.message,
                },
            }));
        }
    }

    async fn apply_disk_store(&mut self, intent: &mut BatchIntent) {
        if let Some(cache) = intent.disk_store.take() {
            self.engine.cache.disk_store = cache;
            self.engine.store_resident_caches().await;
        }
    }

    fn apply_graph_state<C>(&mut self, intent: &mut BatchIntent, callback: &C)
    where
        C: Fn(WorkerReport),
    {
        match intent.graph_state.take() {
            Some(GraphOp::Clear) => {
                self.engine.clear();
                callback(WorkerReport::Cleared);
            }
            Some(GraphOp::Replace(compiled)) => {
                tracing::info!("Graph updated");
                self.engine.install(Arc::clone(&compiled));
                callback(WorkerReport::Installed(compiled));
            }
            None => {}
        }
    }

    async fn evict_cache<C>(&mut self, intent: &mut BatchIntent, callback: &C)
    where
        C: Fn(WorkerReport),
    {
        if intent.evict_cache.is_empty() {
            return;
        }
        let node_ids = std::mem::take(&mut intent.evict_cache)
            .into_iter()
            .collect::<Vec<_>>();
        let failures = self.engine.evict_cache(&node_ids).await;
        if failures.is_empty() {
            return;
        }
        let details = failures
            .iter()
            .map(|failure| format!("{:?}: {}", failure.e_node_id, failure.message))
            .collect::<Vec<_>>()
            .join("; ");
        callback(WorkerReport::Error(WorkerError::CacheEviction {
            failure_count: failures.len(),
            details,
        }));
    }

    async fn execute_intent<C>(&mut self, intent: &mut BatchIntent, callback: &C)
    where
        C: Fn(WorkerReport) + Sync,
    {
        if !self.execution_requested(intent) {
            return;
        }

        self.cancel.reset();
        let cancel = self.cancel.clone();
        self.report_activity(true, callback);
        let _pause_guard = self.event_loop_pause_gate.close();
        let seeds = intent.take_run_seeds();
        let result = self.run_and_forward(seeds, cancel, callback).await;

        match result {
            Ok(()) => {
                let event_triggers = std::mem::take(&mut self.outcome.event_triggers);
                if intent.loop_command == Some(LoopCommand::Start) {
                    self.spawn_event_loop(event_triggers).await;
                }
                let activity = self.activity(false);
                callback(WorkerReport::Status(
                    self.status.completed(activity, &mut self.outcome),
                ));
            }
            Err(error) => {
                self.report_activity(false, callback);
                callback(WorkerReport::Error(WorkerError::Execution { error }));
            }
        }
    }

    fn execution_requested(&self, intent: &BatchIntent) -> bool {
        !self.engine.is_empty()
            && (intent.execute_sinks
                || intent.execute_event_triggers
                || !intent.execute_nodes.is_empty()
                || !intent.events.is_empty()
                || intent.loop_command == Some(LoopCommand::Start))
    }

    fn activity(&self, executing: bool) -> WorkerActivity {
        match (executing, self.event_loop.is_some()) {
            (false, false) => WorkerActivity::Idle,
            (true, false) => WorkerActivity::Executing,
            (false, true) => WorkerActivity::EventLoop,
            (true, true) => WorkerActivity::ExecutingEventLoop,
        }
    }

    fn report_activity<C>(&mut self, executing: bool, callback: &C)
    where
        C: Fn(WorkerReport),
    {
        let activity = self.activity(executing);
        callback(WorkerReport::Status(self.status.activity(activity)));
    }

    async fn spawn_event_loop(&mut self, event_triggers: Vec<EventTrigger>) {
        assert!(self.event_loop.is_none());
        if event_triggers.is_empty() {
            return;
        }
        self.event_loop =
            Some(ActiveEventLoop::spawn(event_triggers, self.event_loop_pause_gate.clone()).await);
        tracing::info!("Event loop started");
    }

    async fn run_and_forward<C>(
        &mut self,
        seeds: RunSeeds,
        cancel: CancelToken,
        callback: &C,
    ) -> Result<()>
    where
        C: Fn(WorkerReport) + Sync,
    {
        self.run_events.clear();
        let (event_tx, mut event_rx) = unbounded_channel::<RunEvent>();
        let engine = &mut self.engine;
        let status = &mut self.status;
        let outcome = &mut self.outcome;
        let events = &mut self.run_events;
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

pub(crate) async fn run<C>(
    messages: UnboundedReceiver<Vec<WorkerMessage>>,
    callback: C,
    cancel: CancelToken,
) where
    C: Fn(WorkerReport) + Send + Sync + 'static,
{
    WorkerTask::new(cancel).process(messages, &callback).await;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use tokio::sync::mpsc;

    use common::CancelToken;

    use crate::execution::Error;
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::report::{RunEvent, RunPhase, RunProgress};
    use crate::worker::event_loop::{LambdaPanic, StopOutcome};
    use crate::worker::protocol::{WorkerError, WorkerReport};
    use crate::worker::status::{NodeExecutionStatus, WorkerActivity, WorkerStatusKind};
    use crate::worker::task::{WorkerTask, forward_run_events};

    fn task() -> WorkerTask {
        WorkerTask::new(CancelToken::new())
    }

    #[test]
    fn event_loop_stop_reports_idle_status_before_lambda_errors() {
        let e_node_id = ExecutionNodeId::unique();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut task = task();
        drop(task.status.activity(WorkerActivity::EventLoop));

        task.report_event_loop_stop(
            StopOutcome {
                was_running: true,
                panics: vec![LambdaPanic {
                    e_node_id,
                    message: "boom".into(),
                }],
            },
            &|report| tx.send(report).unwrap(),
        );

        let WorkerReport::Status(status) = rx.try_recv().unwrap() else {
            panic!("event-loop stop must publish worker status");
        };
        assert_eq!(status.activity, WorkerActivity::Idle);
        assert_eq!(status.kind, WorkerStatusKind::Activity);
        let WorkerReport::Error(WorkerError::Execution { error }) = rx.try_recv().unwrap() else {
            panic!("event-lambda panic must use the general worker error report");
        };
        assert!(matches!(
            error,
            Error::EventLambdaPanic {
                e_node_id: actual,
                message
            } if actual == e_node_id && message == "boom"
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn node_updates_batch_and_published_snapshots_stay_stable() {
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
        let mut task = task();
        drop(task.status.activity(WorkerActivity::Executing));
        let (tx, mut rx) = mpsc::unbounded_channel();

        forward_run_events(&mut events, &mut task.status, &|report| {
            tx.send(report).unwrap()
        });

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

        let idle = task.status.activity(WorkerActivity::Idle);
        assert!(!Arc::ptr_eq(&patch, &idle));
        assert_eq!(patch.activity, WorkerActivity::Executing);
        assert_eq!(patch.nodes.len(), 2);
        assert_eq!(idle.activity, WorkerActivity::Idle);
        assert!(idle.nodes.is_empty());

        drop(patch);
        let allocation = Arc::as_ptr(&idle);
        drop(idle);
        let executing = task.status.activity(WorkerActivity::Executing);
        assert_eq!(Arc::as_ptr(&executing), allocation);
        assert!(rx.try_recv().is_err());
    }
}
