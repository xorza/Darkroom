use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver, UnboundedSender, channel, unbounded_channel};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use common::ReadyState;
use common::pause_gate::PauseGate;

use crate::common::shared_any_state::SharedAnyState;
use crate::event_lambda::EventLambda;
use crate::execution_graph::{ArgumentValues, ExecutionGraph, Result};
use crate::execution_stats::ExecutionStats;
use crate::function::FuncLib;
use crate::graph::{Graph, NodeId};

/// Capacity of the bounded channel each event-lambda task writes into.
/// The worker reads this channel directly and applies backpressure to
/// lambdas when it can't keep up.
const EVENT_LOOP_BACKPRESSURE: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventRef {
    pub node_id: NodeId,
    pub event_idx: usize,
}

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

/// The wire format is `Vec<WorkerMessage>`: one send = one commit unit.
/// Batch atomicity is type-enforced — the worker cannot split a batch
/// across two scan/commit cycles.
#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<Vec<WorkerMessage>>,
    event_loop_started: Arc<AtomicBool>,
}

impl Worker {
    pub fn new<ExecutionCallback>(callback: ExecutionCallback) -> Self
    where
        ExecutionCallback: Fn(Result<ExecutionStats>) + Send + 'static,
    {
        let (tx, rx) = unbounded_channel::<Vec<WorkerMessage>>();
        let event_loop_started = Arc::new(AtomicBool::new(false));
        let thread_handle: JoinHandle<()> = tokio::spawn({
            let event_loop_started = event_loop_started.clone();
            async move {
                worker_loop(rx, callback, event_loop_started).await;
            }
        });

        Self {
            thread_handle: Some(thread_handle),
            tx,
            event_loop_started,
        }
    }

    pub fn is_event_loop_started(&self) -> bool {
        self.event_loop_started.load(Ordering::Relaxed)
    }

    pub fn send(&self, msg: WorkerMessage) {
        self.tx
            .send(vec![msg])
            .expect("Failed to send message to worker, it probably exited");
    }

    pub fn send_many<T: IntoIterator<Item = WorkerMessage>>(&self, msgs: T) {
        let msgs: Vec<WorkerMessage> = msgs.into_iter().collect();
        if !msgs.is_empty() {
            self.tx
                .send(msgs)
                .expect("Failed to send message to worker, it probably exited");
        }
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

#[derive(Debug)]
struct EventLoopHandle {
    join_handles: Vec<JoinHandle<()>>,
}

impl EventLoopHandle {
    /// Cancels every spawned lambda task. Each task's in-flight
    /// `.await` is aborted — lambda authors must write cancel-safe
    /// code. Panics are re-raised on the caller.
    async fn stop(&mut self) {
        for ah in self.join_handles.drain(..) {
            ah.abort();
            if let Err(err) = ah.await {
                if err.is_panic() {
                    std::panic::resume_unwind(err.into_panic());
                } else {
                    assert!(err.is_cancelled(), "event task join error: {err}");
                }
            }
        }
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

/// One `(event, lambda, state)` triple spawned as a looping task.
#[derive(Debug)]
pub struct EventTrigger {
    pub event: EventRef,
    pub lambda: EventLambda,
    pub state: SharedAnyState,
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
) where
    ExecutionCallback: Fn(Result<ExecutionStats>) + Send + 'static,
{
    let mut execution_graph = ExecutionGraph::default();

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
            }
            n = async {
                event_loop.as_mut().unwrap().1
                    .recv_many(&mut ev_buf, EVENT_LOOP_BACKPRESSURE).await
            }, if event_loop.is_some() => {
                if n == 0 {
                    // Event channel closed with loop still nominally
                    // running — lambdas all died. Tear down and keep
                    // going; no stats to report.
                    if let Some((mut handle, _)) = event_loop.take() {
                        handle.stop().await;
                    }
                    continue;
                }
            }
        }

        let event_loop_pause_guard = event_loop_pause_gate.close();

        // --- Scan
        let mut intent = scan(std::mem::take(&mut cmd_batch));
        intent.events.extend(ev_buf.drain(..));

        // --- Commit: stop the loop once if anything touches it, then
        // apply graph op → execute → start loop in stable order.
        let needs_stop =
            intent.graph_state.is_some() || intent.loop_request.is_some() || intent.exit;
        let loop_was_running_before_stop = if needs_stop {
            stop_event_loop(&mut event_loop).await
        } else {
            false
        };

        if intent.exit {
            return;
        }

        match intent.graph_state.take() {
            Some(GraphOp::Clear) => execution_graph.clear(),
            Some(GraphOp::Replace(graph, func_lib)) => {
                tracing::info!("Graph updated");
                execution_graph.update(&graph, &func_lib);
            }
            None => {}
        }
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
        if needs_execute && !execution_graph.is_empty() {
            let in_loop = should_start_event_loop || event_loop.is_some();
            let result = execution_graph
                .execute(intent.execute_terminals, in_loop, intent.events.drain())
                .await;

            if should_start_event_loop && let Ok(stats) = &result {
                assert!(event_loop.is_none());
                let triggers = execution_graph.active_event_triggers(stats);
                if !triggers.is_empty() {
                    event_loop =
                        Some(start_event_loop(triggers, event_loop_pause_gate.clone()).await);
                    tracing::info!("Event loop started");
                }
            }

            (execution_callback)(result);
        }

        for (node_id, reply) in intent.argument_requests.drain(..) {
            let values = execution_graph
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

        drop(event_loop_pause_guard);
        tokio::task::yield_now().await;
    }
}

/// Tears down any running event loop. Dropping the `Receiver` alongside
/// the handle guarantees that any events still in flight die with the
/// old channel — there's no way for them to re-enter the worker.
/// Returns `true` if a loop was actually running.
async fn stop_event_loop(event_loop: &mut Option<(EventLoopHandle, Receiver<EventRef>)>) -> bool {
    match event_loop.take() {
        Some((mut handle, _rx)) => {
            handle.stop().await;
            tracing::info!("Event loop stopped");
            true
        }
        None => false,
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
    let mut join_handles: Vec<JoinHandle<()>> = Vec::default();

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
        join_handles.push(join_handle);
    }

    ready.wait().await;
    tokio::task::yield_now().await;

    (EventLoopHandle { join_handles }, event_rx)
}

#[cfg(test)]
mod tests;
