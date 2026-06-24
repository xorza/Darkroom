use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc::{Receiver, UnboundedReceiver, UnboundedSender, channel, unbounded_channel};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use common::CancelToken;
use common::PauseGate;
use common::ReadyState;

use crate::execution::event::{EventRef, EventTrigger};
use crate::execution::{ArgumentValues, Error, ExecutionEngine, Result, RunSeeds};
use crate::execution_stats::{ExecutionStats, RunProgress};
use crate::function::FuncLib;
use crate::graph::{Graph, NodeId};

/// What the worker reports back to its host: live per-node [`RunProgress`]
/// during a run, then a single [`WorkerReport::Finished`] with the run's full
/// stats (or error). Progress events always precede the matching `Finished`.
#[derive(Debug)]
pub enum WorkerReport {
    Progress(RunProgress),
    Finished(Result<ExecutionStats>),
}

/// Capacity of the bounded channel each event-lambda task writes into.
/// The worker reads this channel directly and applies backpressure to
/// lambdas when it can't keep up.
const EVENT_LOOP_BACKPRESSURE: usize = 10;

/// Command enum sent into the worker loop.
///
/// Cancel-safety: `Update`, `Clear`, `StopEventLoop`, and `Exit` tear
/// down the currently-running event loop, which aborts every lambda
/// task at its next `.await`. Lambda authors must therefore write
/// cancel-safe code — any state held across `.await` can be dropped
/// without cleanup. See [`EventLoopHandle::stop`].
#[derive(Debug)]
pub enum WorkerMessage {
    Exit,
    /// External event injection side-door (scripting, replay, tests).
    /// Events produced by running lambdas reach the worker on the
    /// internal bounded channel, not through this variant.
    InjectEvents {
        events: Vec<EventRef>,
    },

    Update {
        graph: Graph,
        func_lib: Arc<FuncLib>,
    },
    Clear,
    ExecuteTerminals,
    StartEventLoop,
    StopEventLoop,
    /// Reserved external sync primitive: the oneshot fires after every
    /// command sent before this one has committed. No current prod
    /// caller — anticipated use is the script transport.
    Sync {
        reply: oneshot::Sender<()>,
    },
    RequestArgumentValues {
        node_id: NodeId,
        reply: oneshot::Sender<Option<ArgumentValues>>,
    },
}

/// Returned when a send target a worker whose task has already exited
/// (via `Exit` message or abort). Callers can treat this as a
/// harmless no-op on shutdown paths.
#[derive(Debug, thiserror::Error)]
#[error("worker task has exited")]
pub struct WorkerExited;

/// The wire format is `Vec<WorkerMessage>`: one send = one commit unit.
/// Batch atomicity is type-enforced — the worker cannot split a batch
/// across two scan/commit cycles.
#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<Vec<WorkerMessage>>,
    event_loop_started: Arc<AtomicBool>,
    /// Cooperative cancel for the in-flight run: the executor polls it
    /// between nodes, and long ops (via `ContextManager::cancel_flag`) poll it
    /// too. Set via [`Worker::request_cancel`]; cleared at each run's start.
    cancel: CancelToken,
}

impl Worker {
    pub fn new<ExecutionCallback>(callback: ExecutionCallback) -> Self
    where
        ExecutionCallback: Fn(WorkerReport) + Send + 'static,
    {
        let (tx, rx) = unbounded_channel::<Vec<WorkerMessage>>();
        let event_loop_started = Arc::new(AtomicBool::new(false));
        let cancel = CancelToken::new();
        let thread_handle: JoinHandle<()> = tokio::spawn({
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

    /// Request cancellation of the currently-running graph. Coarse: the
    /// in-flight node still finishes, but no further nodes are scheduled and
    /// the run reports `cancelled`. A no-op when nothing is running (cleared
    /// at the next run's start).
    pub fn request_cancel(&self) {
        self.cancel.cancel();
    }

    pub fn send(&self, msg: WorkerMessage) -> std::result::Result<(), WorkerExited> {
        self.tx.send(vec![msg]).map_err(|_| WorkerExited)
    }

    pub fn send_many<T: IntoIterator<Item = WorkerMessage>>(
        &self,
        msgs: T,
    ) -> std::result::Result<(), WorkerExited> {
        let msgs: Vec<WorkerMessage> = msgs.into_iter().collect();
        if msgs.is_empty() {
            return Ok(());
        }
        self.tx.send(msgs).map_err(|_| WorkerExited)
    }

    pub fn exit(&mut self) {
        self.tx.send(vec![WorkerMessage::Exit]).ok();

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

/// A lambda task that ended by panicking, paired with the node it ran for so
/// the worker can attribute the report.
#[derive(Debug)]
struct LambdaPanic {
    node_id: NodeId,
    message: String,
}

/// Best-effort message extraction from a caught panic payload.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[derive(Debug)]
struct EventLoopHandle {
    join_handles: Vec<(EventRef, JoinHandle<()>)>,
}

impl EventLoopHandle {
    /// Cancels every spawned lambda task and joins it. A task that ended by
    /// panicking is **isolated** — its panic is captured and returned (the
    /// worker reports it via the execution callback) rather than unwound into
    /// the worker loop, which would kill the worker. Cancellations (from
    /// `abort`) are expected; any other join error is a bug. Lambda authors
    /// must still write cancel-safe code (aborts happen at the next `.await`).
    async fn stop(&mut self) -> Vec<LambdaPanic> {
        let mut panics = Vec::new();
        for (event, ah) in self.join_handles.drain(..) {
            ah.abort();
            if let Err(err) = ah.await {
                if err.is_panic() {
                    panics.push(LambdaPanic {
                        node_id: event.node_id,
                        message: panic_message(err.into_panic()),
                    });
                } else {
                    assert!(err.is_cancelled(), "event task join error: {err}");
                }
            }
        }
        panics
    }
}

/// Forwards captured lambda panics to the execution callback as `Err`s.
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

/// Graph-state intent after a batch is scanned.
///
/// Reduction rule: last-write-wins. Whichever of `Clear` / `Replace`
/// appears last in send order is the final value.
#[derive(Debug)]
enum GraphOp {
    Clear,
    Replace(Graph, Arc<FuncLib>),
}

/// Event-loop intent after a batch is scanned.
///
/// Reduction rule: last-write-wins. Absence means "leave running
/// state as-is."
#[derive(Debug)]
enum LoopCommand {
    Start,
    Stop,
}

/// Per-iteration accumulator for the worker loop's scan phase. Every
/// command goes into a field here; side effects (stop loop, execute,
/// start loop, run callbacks) happen in the commit phase below.
///
/// Reduction table — when multiple commands in one batch target the
/// same slot, `scan()` collapses them per-slot. Add a row here
/// before adding a new variant that can conflict with an existing
/// one.
///
/// Exit is special: it dominates the entire batch. If Exit appears
/// anywhere in the batch, the scanned intent is pure Exit and every
/// other command in the same batch is discarded — whether sent
/// before or after Exit.
///
/// | Slot                  | Variants that write it      | Rule          |
/// | --------------------- | --------------------------- | ------------- |
/// | `graph_state`         | Update, Clear               | last-write-wins |
/// | `loop_request`        | StartEventLoop, StopEventLoop | last-write-wins |
/// | `execute_terminals`   | ExecuteTerminals            | idempotent flag |
/// | `events`              | InjectEvents                | set union (dedup) |
/// | `syncs`               | Sync                        | all fire      |
/// | `argument_requests`   | RequestArgumentValues       | all fire, post-commit snapshot |
/// | `exit`                | Exit                        | dominates entire batch |
#[derive(Debug, Default)]
struct BatchIntent {
    graph_state: Option<GraphOp>,
    loop_request: Option<LoopCommand>,
    execute_terminals: bool,
    exit: bool,
    events: HashSet<EventRef>,
    syncs: Vec<oneshot::Sender<()>>,
    argument_requests: Vec<(NodeId, oneshot::Sender<Option<ArgumentValues>>)>,
}

/// Pure scan: folds a command batch into a `BatchIntent`. Exit
/// dominates the entire batch: if Exit appears anywhere, the returned
/// intent is pure Exit — every other command in the same batch is
/// discarded (including those before Exit). Oneshot senders in
/// dropped variants close silently; waiters get "sender dropped,"
/// which is the right signal on shutdown.
fn scan(msgs: Vec<WorkerMessage>) -> BatchIntent {
    let mut intent = BatchIntent::default();
    for msg in msgs {
        match msg {
            WorkerMessage::Exit => {
                return BatchIntent {
                    exit: true,
                    ..BatchIntent::default()
                };
            }
            WorkerMessage::InjectEvents { events } => intent.events.extend(events),
            WorkerMessage::Update { graph, func_lib } => {
                intent.graph_state = Some(GraphOp::Replace(graph, func_lib));
            }
            WorkerMessage::Clear => intent.graph_state = Some(GraphOp::Clear),
            WorkerMessage::ExecuteTerminals => intent.execute_terminals = true,
            WorkerMessage::StartEventLoop => intent.loop_request = Some(LoopCommand::Start),
            WorkerMessage::StopEventLoop => intent.loop_request = Some(LoopCommand::Stop),
            WorkerMessage::Sync { reply } => {
                intent.syncs.push(reply);
            }
            WorkerMessage::RequestArgumentValues { node_id, reply } => {
                intent.argument_requests.push((node_id, reply));
            }
        }
    }
    intent
}

async fn worker_loop<ExecutionCallback>(
    mut worker_message_rx: UnboundedReceiver<Vec<WorkerMessage>>,
    execution_callback: ExecutionCallback,
    event_loop_started: Arc<AtomicBool>,
    cancel: CancelToken,
) where
    ExecutionCallback: Fn(WorkerReport) + Send + 'static,
{
    let mut execution_engine = ExecutionEngine::default();

    let mut cmd_batch: Vec<WorkerMessage> = Vec::new();
    let mut ev_buf: Vec<EventRef> = Vec::with_capacity(EVENT_LOOP_BACKPRESSURE);

    let mut event_loop: Option<(EventLoopHandle, Receiver<EventRef>)> = None;
    let event_loop_pause_gate = PauseGate::default();

    loop {
        event_loop_started.store(event_loop.is_some(), Ordering::Relaxed);

        ev_buf.clear();

        // `biased`: commands take priority so Stop/Exit/Clear can't be
        // starved by a torrent of lambda events.
        tokio::select! {
            biased;
            batch = worker_message_rx.recv() => {
                match batch {
                    Some(b) => cmd_batch = b,
                    None => return,
                }
                // Opportunistically fold any additional batches that
                // queued up during the prior commit. Per-slot
                // reduction in `BatchIntent` handles the merge.
                while let Ok(more) = worker_message_rx.try_recv() {
                    cmd_batch.extend(more);
                }
            }
            n = async {
                event_loop.as_mut().unwrap().1
                    .recv_many(&mut ev_buf, EVENT_LOOP_BACKPRESSURE).await
            }, if event_loop.is_some() => {
                if n == 0 {
                    // Event channel closed with loop still nominally
                    // running — lambdas all died (returned or panicked).
                    // Tear down, report any panics, and keep going.
                    if let Some((mut handle, _)) = event_loop.take() {
                        let panics = handle.stop().await;
                        report_lambda_panics(panics, &execution_callback);
                    }
                    continue;
                }
            }
        }

        // --- Scan
        let mut intent = scan(std::mem::take(&mut cmd_batch));
        intent.events.extend(ev_buf.drain(..));

        // --- Commit: stop the loop once if anything touches it, then
        // apply graph op → execute → start loop in stable order.
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

        let update_ok = match intent.graph_state.take() {
            Some(GraphOp::Clear) => {
                execution_engine.clear();
                true
            }
            Some(GraphOp::Replace(graph, func_lib)) => {
                tracing::info!("Graph updated");
                match execution_engine.update(&graph, &func_lib) {
                    Ok(()) => true,
                    Err(e) => {
                        // Compile failed (e.g. a func missing from the lib):
                        // report it like a run failure and skip execute —
                        // the prior program is left untouched by `update`.
                        (execution_callback)(WorkerReport::Finished(Err(e)));
                        false
                    }
                }
            }
            None => true,
        };
        let should_start_event_loop = match intent.loop_request {
            Some(LoopCommand::Start) => true,
            Some(LoopCommand::Stop) => false,
            // No explicit request: preserve the prior running state.
            // Combined with "Update forces a reset," this is what
            // makes an Update restart a running loop on the new graph.
            None => loop_was_running_before_stop,
        };

        let needs_execute =
            intent.execute_terminals || !intent.events.is_empty() || should_start_event_loop;

        // Empty graph is a normal state, not a failure: skip execute
        // silently. Events/terminals/StartEventLoop are no-ops until
        // a graph is loaded.
        if update_ok && needs_execute && !execution_engine.is_empty() {
            // Quiesce the event loop around execute(): closing the gate
            // stops lambdas from *starting a new iteration*. It does NOT
            // pause a lambda already inside `invoke()`, so this is not an
            // atomic cross-node snapshot — execute can still observe
            // per-node `SharedAnyState` that an in-flight lambda is mid-
            // update on another node. Per-node mutexes keep each node's
            // state un-torn; the gate just bounds how much churn races
            // execute. No-op when the loop was torn down above
            // (needs_stop path) or wasn't running.
            let _pause_guard = event_loop_pause_gate.close();

            let in_loop = should_start_event_loop || event_loop.is_some();
            // Fresh run: clear any cancel left set while idle — a cancel only
            // applies to the run in flight when it was requested. The host
            // sets `cancel` directly (a shared token), so no command-channel
            // round-trip is needed for it to be observed mid-run.
            cancel.reset();
            // Forward each node's live progress to the host as it runs, ahead
            // of the final stats: the executor sends on `prog_tx`, and this
            // loop drains `prog_rx` concurrently with the run, batching each
            // wake with `recv_many` to keep select churn down.
            let (prog_tx, mut prog_rx) = unbounded_channel::<RunProgress>();
            let mut prog_buf: Vec<RunProgress> = Vec::new();
            let result = {
                let run = execution_engine.execute(
                    RunSeeds {
                        terminals: intent.execute_terminals,
                        event_triggers: in_loop,
                        events: intent.events.drain().collect(),
                    },
                    Some(&prog_tx),
                    cancel.clone(),
                );
                tokio::pin!(run);
                loop {
                    tokio::select! {
                        biased;
                        r = &mut run => break r,
                        _ = prog_rx.recv_many(&mut prog_buf, 64) => {
                            for p in prog_buf.drain(..) {
                                (execution_callback)(WorkerReport::Progress(p));
                            }
                        }
                    }
                }
            };
            // Flush any progress buffered between the last poll and completion;
            // the dropped sender lets `recv_many` return all remaining at once.
            drop(prog_tx);
            prog_rx.recv_many(&mut prog_buf, usize::MAX).await;
            for p in prog_buf.drain(..) {
                (execution_callback)(WorkerReport::Progress(p));
            }

            if should_start_event_loop && let Ok(stats) = &result {
                assert!(event_loop.is_none());
                let triggers = execution_engine.active_event_triggers(stats);
                if !triggers.is_empty() {
                    event_loop =
                        Some(start_event_loop(triggers, event_loop_pause_gate.clone()).await);
                    tracing::info!("Event loop started");
                }
            }

            (execution_callback)(WorkerReport::Finished(result));
        }

        for (node_id, reply) in intent.argument_requests.drain(..) {
            let values = execution_engine
                .get_argument_values_with_previews(&node_id)
                .await;
            let _ = reply.send(values);
        }

        // Refresh the atomic so callers racing a Sync reply see the
        // post-commit state.
        event_loop_started.store(event_loop.is_some(), Ordering::Relaxed);

        for reply in intent.syncs.drain(..) {
            let _ = reply.send(());
        }

        tokio::task::yield_now().await;
    }
}

/// Outcome of a teardown: whether a loop was running, and any lambda panics
/// captured while joining (for the worker to report).
#[derive(Debug, Default)]
struct StopOutcome {
    was_running: bool,
    panics: Vec<LambdaPanic>,
}

/// Tears down any running event loop. Dropping the `Receiver` alongside
/// the handle guarantees that any events still in flight die with the
/// old channel — there's no way for them to re-enter the worker.
async fn stop_event_loop(
    event_loop: &mut Option<(EventLoopHandle, Receiver<EventRef>)>,
) -> StopOutcome {
    match event_loop.take() {
        Some((mut handle, _rx)) => {
            let panics = handle.stop().await;
            tracing::info!("Event loop stopped");
            StopOutcome {
                was_running: true,
                panics,
            }
        }
        None => StopOutcome::default(),
    }
}

/// Spawns one task per `EventTrigger`, each looping
/// `lambda → send → pause-gate-wait → yield`. Returns the handle plus
/// the receiving half of the bounded event channel — the worker reads
/// events from it directly, no forwarder in between.
async fn start_event_loop(
    event_triggers: Vec<EventTrigger>,
    pause_gate: PauseGate,
) -> (EventLoopHandle, Receiver<EventRef>) {
    assert!(!event_triggers.is_empty());

    let (event_tx, event_rx) = channel::<EventRef>(EVENT_LOOP_BACKPRESSURE);
    let ready = ReadyState::new(event_triggers.len());
    let mut join_handles: Vec<(EventRef, JoinHandle<()>)> = Vec::default();

    for EventTrigger {
        event,
        lambda,
        state,
    } in event_triggers
    {
        let join_handle = tokio::spawn({
            let event_tx = event_tx.clone();
            let ready = ready.clone();
            let pause_gate = pause_gate.clone();

            async move {
                ready.signal();

                loop {
                    lambda.invoke(state.clone()).await;
                    if event_tx.send(event).await.is_err() {
                        return;
                    }
                    pause_gate.wait().await;
                    tokio::task::yield_now().await;
                }
            }
        });
        // `event` is `Copy`, so the spawned task and this pairing each keep one.
        join_handles.push((event, join_handle));
    }

    ready.wait().await;
    tokio::task::yield_now().await;

    (EventLoopHandle { join_handles }, event_rx)
}

#[cfg(test)]
mod tests;
