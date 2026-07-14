use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

use common::{CancelToken, PauseGate};

use crate::execution::report::RunEvent;
use crate::execution::stats::ExecutionStats;
use crate::execution::{Error, ExecutionEngine, Result, RunSeeds};
use crate::worker::batch::{GraphOp, LoopCommand, scan};
use crate::worker::event_loop::{
    ActiveEventLoop, EVENT_LOOP_BACKPRESSURE, LambdaPanic, StopOutcome, stop_event_loop,
};
use crate::worker::protocol::{WorkerExited, WorkerMessage, WorkerReport};

pub(crate) mod batch;
pub(crate) mod event_loop;
pub(crate) mod protocol;

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<Vec<WorkerMessage>>,
    event_loop_started: Arc<AtomicBool>,
    cancel: CancelToken,
}

impl Worker {
    pub fn new<ExecutionCallback>(callback: ExecutionCallback) -> Self
    where
        ExecutionCallback: Fn(WorkerReport) + Send + Sync + 'static,
    {
        let (tx, rx) = unbounded_channel::<Vec<WorkerMessage>>();
        let event_loop_started = Arc::new(AtomicBool::new(false));
        let cancel = CancelToken::new();
        let thread_handle = tokio::spawn({
            let event_loop_started = event_loop_started.clone();
            let cancel = cancel.clone();
            async move {
                worker_loop(rx, callback, event_loop_started, cancel).await;
            }
        });

        Self {
            thread_handle: Some(thread_handle),
            tx,
            event_loop_started,
            cancel,
        }
    }

    pub fn is_event_loop_started(&self) -> bool {
        self.event_loop_started.load(Ordering::Relaxed)
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

fn report_lambda_panics<C>(panics: Vec<LambdaPanic>, callback: &C)
where
    C: Fn(WorkerReport),
{
    for panic in panics {
        callback(WorkerReport::Finished(Err(Error::EventLambdaPanic {
            node_id: panic.node_id,
            message: panic.message,
        })));
    }
}

async fn worker_loop<ExecutionCallback>(
    mut worker_message_rx: UnboundedReceiver<Vec<WorkerMessage>>,
    execution_callback: ExecutionCallback,
    event_loop_started: Arc<AtomicBool>,
    cancel: CancelToken,
) where
    ExecutionCallback: Fn(WorkerReport) + Send + Sync + 'static,
{
    let mut execution_engine = ExecutionEngine::default();
    let mut cmd_batch = Vec::new();
    let mut event_buffer = Vec::with_capacity(EVENT_LOOP_BACKPRESSURE);
    let mut event_loop: Option<ActiveEventLoop> = None;
    let event_loop_pause_gate = PauseGate::default();

    loop {
        event_loop_started.store(event_loop.is_some(), Ordering::Relaxed);
        event_buffer.clear();

        tokio::select! {
            biased;
            batch = worker_message_rx.recv() => {
                match batch {
                    Some(batch) => cmd_batch = batch,
                    None => return,
                }
                while let Ok(more) = worker_message_rx.try_recv() {
                    cmd_batch.extend(more);
                }
            }
            count = async {
                event_loop.as_mut().unwrap().events
                    .recv_many(&mut event_buffer, EVENT_LOOP_BACKPRESSURE).await
            }, if event_loop.is_some() => {
                if count == 0 {
                    let mut active = event_loop.take().unwrap();
                    report_lambda_panics(active.stop().await, &execution_callback);
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
        report_lambda_panics(stop_outcome.panics, &execution_callback);
        let loop_was_running_before_stop = stop_outcome.was_running;

        if intent.exit {
            return;
        }

        if let Some(cache) = intent.disk_store.take() {
            execution_engine.set_disk_store(cache);
            execution_engine.store_resident_caches().await;
        }

        match intent.graph_state.take() {
            Some(GraphOp::Clear) => execution_engine.clear(),
            Some(GraphOp::Replace(compiled)) => {
                tracing::info!("Graph updated");
                execution_engine.install(compiled);
            }
            None => {}
        }

        if intent.save_caches && !execution_engine.is_empty() {
            execution_engine.store_resident_caches().await;
        }

        let should_start_event_loop = match intent.loop_request {
            Some(LoopCommand::Start) => true,
            Some(LoopCommand::Stop) => false,
            None => loop_was_running_before_stop,
        };

        let needs_execute = intent.execute_sinks
            || !intent.execute_nodes.values.is_empty()
            || !intent.events.values.is_empty()
            || should_start_event_loop;

        if needs_execute && !execution_engine.is_empty() {
            let _pause_guard = event_loop_pause_gate.close();
            let in_loop = should_start_event_loop || event_loop.is_some();
            cancel.reset();
            let seeds = RunSeeds {
                sinks: intent.execute_sinks,
                event_triggers: in_loop,
                events: intent.events.take(),
                nodes: intent.execute_nodes.take(),
            };
            let result = run_and_forward(
                &mut execution_engine,
                seeds,
                cancel.clone(),
                &execution_callback,
            )
            .await;

            if should_start_event_loop && let Ok(stats) = &result {
                assert!(event_loop.is_none());
                let triggers = execution_engine.active_event_triggers(stats);
                if !triggers.is_empty() {
                    event_loop =
                        Some(ActiveEventLoop::start(triggers, event_loop_pause_gate.clone()).await);
                    tracing::info!("Event loop started");
                }
            }

            execution_callback(WorkerReport::Finished(result));
        }

        event_loop_started.store(event_loop.is_some(), Ordering::Relaxed);
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
