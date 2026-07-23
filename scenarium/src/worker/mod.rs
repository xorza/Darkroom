use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

use common::CancelToken;

use crate::execution::outcome::ExecutionOutcome;
use crate::execution::report::RunEvent;
use crate::execution::{Error, ExecutionEngine, Result, RunSeeds};
use crate::worker::batch::{GraphOp, LoopCommand, scan};
use crate::worker::event_loop::{
    ActiveEventLoop, EVENT_LOOP_BACKPRESSURE, StopOutcome, stop_event_loop,
};
use crate::worker::pause_gate::PauseGate;
use crate::worker::protocol::{WorkerError, WorkerExited, WorkerMessage, WorkerReport};
use crate::worker::status::{WorkerActivity, WorkerStatusPublisher};

pub(crate) mod batch;
pub(crate) mod event_loop;
pub(crate) mod pause_gate;
pub(crate) mod protocol;
pub(crate) mod status;

const RUN_EVENT_BATCH_SIZE: usize = 64;

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<Vec<WorkerMessage>>,
    cancel: CancelToken,
}

impl Worker {
    pub fn new<ExecutionCallback>(callback: ExecutionCallback) -> Self
    where
        ExecutionCallback: Fn(WorkerReport) + Send + Sync + 'static,
    {
        let (tx, rx) = unbounded_channel::<Vec<WorkerMessage>>();
        let cancel = CancelToken::new();
        let thread_handle = tokio::spawn({
            let cancel = cancel.clone();
            async move {
                worker_loop(rx, callback, cancel).await;
            }
        });

        Self {
            thread_handle: Some(thread_handle),
            tx,
            cancel,
        }
    }

    pub fn request_cancel(&self) {
        self.cancel.cancel();
    }

    pub fn send(&self, msg: WorkerMessage) -> std::result::Result<(), WorkerExited> {
        self.send_many([msg])
    }

    pub fn send_many<T: IntoIterator<Item = WorkerMessage>>(
        &self,
        msgs: T,
    ) -> std::result::Result<(), WorkerExited> {
        let msgs = msgs.into_iter().collect::<Vec<_>>();
        if msgs.is_empty() {
            return Ok(());
        }
        self.tx.send(msgs).map_err(|_| WorkerExited)
    }

    pub fn exit(&mut self) {
        self.tx.send(vec![WorkerMessage::Exit]).ok();
        self.request_cancel();

        if let Some(thread_handle) = self.thread_handle.take() {
            thread_handle.abort();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.exit();
    }
}

fn report_event_loop_stop<C>(outcome: StopOutcome, status: &mut WorkerStatusPublisher, callback: &C)
where
    C: Fn(WorkerReport),
{
    if outcome.was_running {
        callback(WorkerReport::Status(status.activity(WorkerActivity::Idle)));
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

async fn worker_loop<ExecutionCallback>(
    mut worker_message_rx: UnboundedReceiver<Vec<WorkerMessage>>,
    execution_callback: ExecutionCallback,
    cancel: CancelToken,
) where
    ExecutionCallback: Fn(WorkerReport) + Send + Sync + 'static,
{
    let mut execution_engine = ExecutionEngine::default();
    let mut status = WorkerStatusPublisher::default();
    let mut outcome = ExecutionOutcome::default();
    let mut cmd_batch = Vec::new();
    let mut event_buffer = Vec::with_capacity(EVENT_LOOP_BACKPRESSURE);
    let mut run_events = Vec::with_capacity(RUN_EVENT_BATCH_SIZE);
    let mut event_loop: Option<ActiveEventLoop> = None;
    let event_loop_pause_gate = PauseGate::default();

    loop {
        event_buffer.clear();

        tokio::select! {
            biased;
            batch = worker_message_rx.recv() => {
                match batch {
                    Some(batch) => cmd_batch = batch,
                    None => return,
                }
            }
            count = async {
                event_loop.as_mut().unwrap().events
                    .recv_many(&mut event_buffer, EVENT_LOOP_BACKPRESSURE).await
            }, if event_loop.is_some() => {
                if count == 0 {
                    let stop_outcome = stop_event_loop(&mut event_loop).await;
                    report_event_loop_stop(
                        stop_outcome,
                        &mut status,
                        &execution_callback,
                    );
                    continue;
                }
            }
        }

        let mut intent = scan(std::mem::take(&mut cmd_batch));
        intent.events.extend(event_buffer.drain(..));

        let needs_stop =
            intent.graph_state.is_some() || intent.loop_request.is_some() || intent.exit;
        let stop_outcome = if needs_stop {
            stop_event_loop(&mut event_loop).await
        } else {
            StopOutcome::default()
        };
        let loop_was_running_before_stop = stop_outcome.was_running;
        report_event_loop_stop(stop_outcome, &mut status, &execution_callback);

        if intent.exit {
            return;
        }

        if let Some(cache) = intent.disk_store.take() {
            execution_engine.cache.disk_store = cache;
            execution_engine.store_resident_caches().await;
        }

        match intent.graph_state.take() {
            Some(GraphOp::Clear) => {
                execution_engine.clear();
                execution_callback(WorkerReport::Cleared);
            }
            Some(GraphOp::Replace(compiled)) => {
                tracing::info!("Graph updated");
                execution_engine.install(Arc::clone(&compiled));
                execution_callback(WorkerReport::Installed(compiled));
            }
            None => {}
        }

        if !intent.evict_cache.is_empty() {
            let node_ids = std::mem::take(&mut intent.evict_cache)
                .into_iter()
                .collect::<Vec<_>>();
            let failures = execution_engine.evict_cache(&node_ids).await;
            if !failures.is_empty() {
                let details = failures
                    .iter()
                    .map(|failure| format!("{:?}: {}", failure.e_node_id, failure.message))
                    .collect::<Vec<_>>()
                    .join("; ");
                execution_callback(WorkerReport::Error(WorkerError::CacheEviction {
                    failure_count: failures.len(),
                    details,
                }));
            }
        }

        let should_start_event_loop = match intent.loop_request {
            Some(LoopCommand::Start) => true,
            Some(LoopCommand::Stop) => false,
            None => loop_was_running_before_stop,
        };

        let needs_execute = intent.execute_sinks
            || intent.execute_event_sources
            || !intent.execute_nodes.is_empty()
            || !intent.events.is_empty()
            || should_start_event_loop;

        if needs_execute && !execution_engine.is_empty() {
            cancel.reset();
            execution_callback(WorkerReport::Status(
                status.activity(worker_activity(true, event_loop.is_some())),
            ));
            let _pause_guard = event_loop_pause_gate.close();
            let seeds = RunSeeds {
                sinks: intent.execute_sinks,
                event_sources: should_start_event_loop || intent.execute_event_sources,
                events: std::mem::take(&mut intent.events).into_iter().collect(),
                nodes: std::mem::take(&mut intent.execute_nodes)
                    .into_iter()
                    .collect(),
            };
            let result = run_and_forward(
                &mut execution_engine,
                &mut outcome,
                &mut run_events,
                &mut status,
                seeds,
                cancel.clone(),
                &execution_callback,
            )
            .await;

            match result {
                Ok(()) => {
                    let triggers = std::mem::take(&mut outcome.event_triggers);
                    if should_start_event_loop {
                        assert!(event_loop.is_none());
                        if !triggers.is_empty() {
                            event_loop = Some(
                                ActiveEventLoop::start(triggers, event_loop_pause_gate.clone())
                                    .await,
                            );
                            tracing::info!("Event loop started");
                        }
                    }
                    let activity = worker_activity(false, event_loop.is_some());
                    execution_callback(WorkerReport::Status(
                        status.completed(activity, &mut outcome),
                    ));
                }
                Err(error) => {
                    let activity = worker_activity(false, event_loop.is_some());
                    execution_callback(WorkerReport::Status(status.activity(activity)));
                    execution_callback(WorkerReport::Error(WorkerError::Execution { error }));
                }
            }
        }

        for reply in intent.syncs.drain(..) {
            let _ = reply.send(());
        }
        tokio::task::yield_now().await;
    }
}

fn worker_activity(executing: bool, event_loop_active: bool) -> WorkerActivity {
    match (executing, event_loop_active) {
        (false, false) => WorkerActivity::Idle,
        (true, false) => WorkerActivity::Executing,
        (false, true) => WorkerActivity::EventLoop,
        (true, true) => WorkerActivity::ExecutingEventLoop,
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
mod tests;
