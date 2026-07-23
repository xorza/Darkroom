use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

use common::CancelToken;

use crate::execution::report::RunEvent;
use crate::execution::stats::ExecutionStats;
use crate::execution::{Error, ExecutionEngine, Result, RunSeeds};
use crate::worker::batch::{GraphOp, LoopCommand, scan};
use crate::worker::event_loop::{
    ActiveEventLoop, EVENT_LOOP_BACKPRESSURE, StopOutcome, stop_event_loop,
};
use crate::worker::pause_gate::PauseGate;
use crate::worker::protocol::{
    WorkerError, WorkerExited, WorkerLifecycle, WorkerMessage, WorkerReport,
};

pub(crate) mod batch;
pub(crate) mod event_loop;
pub(crate) mod pause_gate;
pub(crate) mod protocol;

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<Vec<WorkerMessage>>,
    active_cancel: Arc<Mutex<Option<CancelToken>>>,
}

impl Worker {
    pub fn new<ExecutionCallback>(callback: ExecutionCallback) -> Self
    where
        ExecutionCallback: Fn(WorkerReport) + Send + Sync + 'static,
    {
        let (tx, rx) = unbounded_channel::<Vec<WorkerMessage>>();
        let active_cancel = Arc::new(Mutex::new(None));
        let thread_handle = tokio::spawn({
            let active_cancel = Arc::clone(&active_cancel);
            async move {
                worker_loop(rx, callback, active_cancel).await;
            }
        });

        Self {
            thread_handle: Some(thread_handle),
            tx,
            active_cancel,
        }
    }

    pub fn request_cancel(&self) {
        if let Some(cancel) = self
            .active_cancel
            .lock()
            .expect("worker cancellation mutex poisoned")
            .as_ref()
        {
            cancel.cancel();
        }
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

fn report_event_loop_stop<C>(outcome: StopOutcome, callback: &C)
where
    C: Fn(WorkerReport),
{
    if outcome.was_running {
        callback(WorkerReport::Lifecycle(WorkerLifecycle::EventLoopStopped));
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
    active_cancel: Arc<Mutex<Option<CancelToken>>>,
) where
    ExecutionCallback: Fn(WorkerReport) + Send + Sync + 'static,
{
    let mut execution_engine = ExecutionEngine::default();
    let mut cmd_batch = Vec::new();
    let mut event_buffer = Vec::with_capacity(EVENT_LOOP_BACKPRESSURE);
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
                    report_event_loop_stop(stop_outcome, &execution_callback);
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
        report_event_loop_stop(stop_outcome, &execution_callback);

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
            || intent.execute_event_triggers
            || !intent.execute_nodes.is_empty()
            || !intent.events.is_empty()
            || should_start_event_loop;

        if needs_execute && !execution_engine.is_empty() {
            execution_callback(WorkerReport::Lifecycle(WorkerLifecycle::ExecutionStarted));
            let _pause_guard = event_loop_pause_gate.close();
            let in_loop = should_start_event_loop || event_loop.is_some();
            let cancel = CancelToken::new();
            *active_cancel
                .lock()
                .expect("worker cancellation mutex poisoned") = Some(cancel.clone());
            let seeds = RunSeeds {
                sinks: intent.execute_sinks,
                event_triggers: in_loop || intent.execute_event_triggers,
                events: std::mem::take(&mut intent.events).into_iter().collect(),
                nodes: std::mem::take(&mut intent.execute_nodes)
                    .into_iter()
                    .collect(),
            };
            let result = run_and_forward(
                &mut execution_engine,
                seeds,
                cancel.clone(),
                &execution_callback,
            )
            .await;
            active_cancel
                .lock()
                .expect("worker cancellation mutex poisoned")
                .take();
            execution_callback(WorkerReport::Lifecycle(WorkerLifecycle::ExecutionStopped));

            match result {
                Ok(stats) => {
                    if should_start_event_loop {
                        assert!(event_loop.is_none());
                        let triggers = execution_engine.active_event_triggers(&stats);
                        if !triggers.is_empty() {
                            event_loop = Some(
                                ActiveEventLoop::start(triggers, event_loop_pause_gate.clone())
                                    .await,
                            );
                            execution_callback(WorkerReport::Lifecycle(
                                WorkerLifecycle::EventLoopStarted,
                            ));
                            tracing::info!("Event loop started");
                        }
                    }
                    execution_callback(WorkerReport::Finished(stats));
                }
                Err(error) => {
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

async fn run_and_forward<C>(
    engine: &mut ExecutionEngine,
    seeds: RunSeeds,
    cancel: CancelToken,
    callback: &C,
) -> Result<ExecutionStats>
where
    C: Fn(WorkerReport) + Sync,
{
    let (event_tx, mut event_rx) = unbounded_channel::<RunEvent>();
    let mut event_buf = Vec::new();
    let report_of = |event: RunEvent| match event {
        RunEvent::Progress(progress) => WorkerReport::Progress(progress),
        RunEvent::PinnedOutputs(outputs) => WorkerReport::PinnedOutputs(outputs),
    };
    let result = {
        let run = engine.execute(seeds, Some(&event_tx), cancel);
        tokio::pin!(run);
        loop {
            tokio::select! {
                biased;
                result = &mut run => break result,
                _ = event_rx.recv_many(&mut event_buf, 64) => {
                    for event in event_buf.drain(..) {
                        callback(report_of(event));
                    }
                }
            }
        }
    };
    drop(event_tx);
    event_rx.recv_many(&mut event_buf, usize::MAX).await;
    for event in event_buf.drain(..) {
        callback(report_of(event));
    }
    result
}

#[cfg(test)]
mod tests;
