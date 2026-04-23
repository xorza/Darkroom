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
use crate::execution_graph::{ArgumentValues, Error, ExecutionGraph, Result};
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
    Event {
        event: EventRef,
    },
    Events {
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
    Ack {
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
/// command goes into a field here; side effects (reset, execute,
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
/// | `events`              | Event, Events               | set union (dedup) |
/// | `acks`                | Ack                         | all fire      |
/// | `argument_requests`   | RequestArgumentValues       | all fire, post-commit snapshot |
/// | `exit`                | Exit                        | dominates entire batch |
#[derive(Debug, Default)]
struct BatchIntent {
    graph_state: Option<GraphOp>,
    loop_request: Option<LoopCommand>,
    execute_terminals: bool,
    exit: bool,
    events: HashSet<EventRef>,
    acks: Vec<oneshot::Sender<()>>,
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
            WorkerMessage::Event { event } => {
                intent.events.insert(event);
            }
            WorkerMessage::Events { events } => intent.events.extend(events),
            WorkerMessage::Update { graph, func_lib } => {
                intent.graph_state = Some(GraphOp::Replace(graph, func_lib));
            }
            WorkerMessage::Clear => intent.graph_state = Some(GraphOp::Clear),
            WorkerMessage::ExecuteTerminals => intent.execute_terminals = true,
            WorkerMessage::StartEventLoop => intent.loop_request = Some(LoopCommand::Start),
            WorkerMessage::StopEventLoop => intent.loop_request = Some(LoopCommand::Stop),
            WorkerMessage::Ack { reply } => {
                intent.acks.push(reply);
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

        // --- Commit: reset the loop once if anything touches it, then
        // apply graph op → execute → start loop in stable order.
        let needs_reset =
            intent.graph_state.is_some() || intent.loop_request.is_some() || intent.exit;
        let was_running = if needs_reset {
            reset_event_loop(&mut event_loop).await
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
            None => was_running,
        };

        if intent.execute_terminals || !intent.events.is_empty() {
            if execution_graph.is_empty() {
                (execution_callback)(Err(Error::EmptyGraph));
            } else {
                let result = execution_graph
                    .execute(
                        intent.execute_terminals,
                        event_loop.is_some(),
                        intent.events.drain(),
                    )
                    .await;
                (execution_callback)(result);
            }
        }

        if should_start_event_loop {
            assert!(event_loop.is_none());

            if execution_graph.is_empty() {
                (execution_callback)(Err(Error::EmptyGraph));
            } else {
                let result = execution_graph.execute(false, true, []).await;

                if let Ok(execution_stats) = &result {
                    let event_triggers = execution_graph.active_event_triggers(execution_stats);

                    if !event_triggers.is_empty() {
                        event_loop = Some(
                            start_event_loop(event_triggers, event_loop_pause_gate.clone()).await,
                        );
                        tracing::info!("Event loop started");
                    }
                };

                (execution_callback)(result);
            }
        }

        for (node_id, reply) in intent.argument_requests.drain(..) {
            let values = execution_graph
                .get_argument_values_with_previews(&node_id)
                .await;
            let _ = reply.send(values);
        }

        // Refresh the atomic so callers racing an Ack reply see the
        // post-commit state.
        event_loop_started.store(event_loop.is_some(), Ordering::Relaxed);

        for reply in intent.acks.drain(..) {
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
async fn reset_event_loop(event_loop: &mut Option<(EventLoopHandle, Receiver<EventRef>)>) -> bool {
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
mod tests {
    use std::sync::Arc;

    use common::output_stream::OutputStream;
    use common::pause_gate::PauseGate;
    use tokio::sync::{Notify, oneshot};
    use tokio::time::{Duration, timeout};

    use crate::common::shared_any_state::SharedAnyState;
    use crate::elements::basic_funclib::BasicFuncLib;
    use crate::elements::worker_events_funclib::WorkerEventsFuncLib;
    use crate::event_lambda::EventLambda;
    use crate::execution_graph::Error;
    use crate::function::FuncLib;
    use crate::graph::{Graph, Node, NodeId};

    use crate::worker::{EventRef, EventTrigger, Worker, WorkerMessage};

    fn log_frame_no_graph(func_lib: &FuncLib) -> Graph {
        let mut graph = Graph::default();

        let frame_event_node_id: NodeId = "e69c3f32-ac66-4447-a3f6-9e8528c5d830".into();
        let float_to_string_node_id: NodeId = "eb6590aa-229d-4874-abba-37c56f5b97fa".into();
        let print_node_id: NodeId = "8be72298-dece-4a5f-8a1d-d2dee1e791d3".into();

        let frame_event_func = func_lib.by_name("frame event").unwrap();
        let float_to_string_func = func_lib.by_name("float to string").unwrap();
        let print_func = func_lib.by_name("print").unwrap();

        let mut frame_event_node: Node = frame_event_func.into();
        frame_event_node.id = frame_event_node_id;
        frame_event_node.inputs[0].binding = 1.into();
        frame_event_node.events[0].subscribers.push(print_node_id);
        graph.add(frame_event_node);

        let mut float_to_string_node: Node = float_to_string_func.into();
        float_to_string_node.id = float_to_string_node_id;
        float_to_string_node.inputs[0].binding = (frame_event_node_id, 1).into();
        graph.add(float_to_string_node);

        let mut print_node: Node = print_func.into();
        print_node.id = print_node_id;
        print_node.inputs[0].binding = (float_to_string_node_id, 0).into();
        graph.add(print_node);

        graph
    }

    #[tokio::test]
    async fn test_worker() -> anyhow::Result<()> {
        let output_stream = OutputStream::new();

        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx
                .try_send(result)
                .expect("Failed to send a compute callback event");
        });

        worker.send_many([
            WorkerMessage::Update {
                graph: graph.clone(),
                func_lib: Arc::new(func_lib.clone()),
            },
            WorkerMessage::Event {
                event: EventRef {
                    node_id: frame_event_node_id,
                    event_idx: 0,
                },
            },
        ]);

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes.len(), 3);
        assert_eq!(output_stream.take().await, ["1"]);

        worker.send(WorkerMessage::Event {
            event: EventRef {
                node_id: frame_event_node_id,
                event_idx: 0,
            },
        });

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        // frame_no is incremented in the update lambda, so each execution increments it
        assert_eq!(executed.executed_nodes.len(), 3);
        assert_eq!(output_stream.take().await, ["2"]);

        worker.send(WorkerMessage::Event {
            event: EventRef {
                node_id: frame_event_node_id,
                event_idx: 0,
            },
        });

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes.len(), 3);
        assert_eq!(output_stream.take().await, ["3"]);

        worker.exit();

        Ok(())
    }

    #[tokio::test]
    async fn start_event_loop_forwards_events() {
        let node_id = NodeId::unique();
        let event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
        let event_state = SharedAnyState::default();

        let (mut handle, mut event_rx) = super::start_event_loop(
            vec![EventTrigger {
                event: EventRef {
                    node_id,
                    event_idx: 0,
                },
                lambda: event_lambda,
                state: event_state,
            }],
            PauseGate::default(),
        )
        .await;

        let event = event_rx.recv().await.expect("Expected event loop event");
        assert_eq!(
            event,
            EventRef {
                node_id,
                event_idx: 0
            }
        );

        handle.stop().await;
    }

    #[tokio::test]
    async fn start_event_loop_waits_for_callback() {
        let node_id = NodeId::unique();
        let notify = Arc::new(Notify::new());
        let notify_for_event = Arc::clone(&notify);
        let event_lambda = EventLambda::new(move |_state| {
            let notify = Arc::clone(&notify_for_event);
            Box::pin(async move {
                notify.notified().await;
            })
        });
        let event_state = SharedAnyState::default();

        let notify_for_callback = Arc::clone(&notify);

        let (mut handle, mut event_rx) = super::start_event_loop(
            vec![EventTrigger {
                event: EventRef {
                    node_id,
                    event_idx: 0,
                },
                lambda: event_lambda,
                state: event_state,
            }],
            PauseGate::default(),
        )
        .await;

        notify_for_callback.notify_waiters();

        let event = timeout(Duration::from_millis(200), event_rx.recv())
            .await
            .expect("Expected event")
            .expect("Event channel closed");
        assert_eq!(
            event,
            EventRef {
                node_id,
                event_idx: 0
            }
        );

        handle.stop().await;
    }

    #[tokio::test]
    async fn pause_gate_blocks_event_loop_iterations() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let node_id = NodeId::unique();
        let invoke_count = Arc::new(AtomicUsize::new(0));
        let invoke_count_clone = Arc::clone(&invoke_count);

        let event_lambda = EventLambda::new(move |_state| {
            let invoke_count = Arc::clone(&invoke_count_clone);
            Box::pin(async move {
                invoke_count.fetch_add(1, Ordering::SeqCst);
            })
        });
        let event_state = SharedAnyState::default();

        let pause_gate = PauseGate::default();

        let (mut handle, mut event_rx) = super::start_event_loop(
            vec![EventTrigger {
                event: EventRef {
                    node_id,
                    event_idx: 0,
                },
                lambda: event_lambda,
                state: event_state,
            }],
            pause_gate.clone(),
        )
        .await;

        // Wait for first event to arrive
        let _ = timeout(Duration::from_millis(100), event_rx.recv())
            .await
            .expect("Expected first event");

        // Close the gate - event loop should pause
        let _guard = pause_gate.close();

        // Record count after closing gate
        tokio::time::sleep(Duration::from_millis(20)).await;
        let count_at_close = invoke_count.load(Ordering::SeqCst);

        // Wait and verify no new invocations while gate is closed
        tokio::time::sleep(Duration::from_millis(100)).await;
        let count_while_closed = invoke_count.load(Ordering::SeqCst);

        // At most one more invocation might have slipped through
        assert!(
            count_while_closed <= count_at_close + 1,
            "Event loop should pause when gate is closed. Count at close: {}, count while closed: {}",
            count_at_close,
            count_while_closed
        );

        // Drop guard to reopen gate
        drop(_guard);

        // Wait for more events to flow
        tokio::time::sleep(Duration::from_millis(100)).await;
        let count_after_reopen = invoke_count.load(Ordering::SeqCst);

        assert!(
            count_after_reopen > count_while_closed,
            "Event loop should resume after gate reopens. Count while closed: {}, count after reopen: {}",
            count_while_closed,
            count_after_reopen
        );

        handle.stop().await;
    }

    #[tokio::test]
    async fn clear_resets_execution_graph() {
        let output_stream = OutputStream::new();

        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        // Setup and execute once
        worker.send_many([
            WorkerMessage::Update {
                graph: graph.clone(),
                func_lib: Arc::new(func_lib.clone()),
            },
            WorkerMessage::Event {
                event: EventRef {
                    node_id: frame_event_node_id,
                    event_idx: 0,
                },
            },
        ]);

        let _ = compute_finish_rx.recv().await;
        assert_eq!(output_stream.take().await, ["1"]);

        // Clear the graph
        worker.send(WorkerMessage::Clear);

        // Try to execute - should not produce output since graph is cleared
        worker.send(WorkerMessage::Event {
            event: EventRef {
                node_id: frame_event_node_id,
                event_idx: 0,
            },
        });

        // Give it time to process - no callback expected since graph is clear
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(output_stream.take().await.is_empty());

        worker.exit();
    }

    #[tokio::test]
    async fn events_are_deduplicated() {
        let output_stream = OutputStream::new();

        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send(WorkerMessage::Update {
            graph: graph.clone(),
            func_lib: Arc::new(func_lib.clone()),
        });

        // Send the same event multiple times in one batch
        let event = EventRef {
            node_id: frame_event_node_id,
            event_idx: 0,
        };
        worker.send(WorkerMessage::Events {
            events: vec![event, event, event],
        });

        let _ = compute_finish_rx.recv().await;

        // Should only print once since duplicate events are deduplicated
        assert_eq!(output_stream.take().await, ["1"]);

        worker.exit();
    }

    #[tokio::test]
    async fn execute_terminals_triggers_terminal_nodes() {
        use crate::data::StaticValue;

        let output_stream = OutputStream::new();

        let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;
        let func_lib = basic_invoker.into_func_lib();

        // Create a simple graph with a terminal print node
        let mut graph = Graph::default();
        let print_func = func_lib.by_name("print").unwrap();

        let mut print_node: Node = print_func.into();
        print_node.id = NodeId::unique();
        // print function is already terminal by definition
        print_node.inputs[0].binding = StaticValue::String("hello".to_string()).into();
        graph.add(print_node);

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send_many([
            WorkerMessage::Update {
                graph: graph.clone(),
                func_lib: Arc::new(func_lib.clone()),
            },
            WorkerMessage::ExecuteTerminals,
        ]);

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes.len(), 1);
        assert_eq!(output_stream.take().await, ["hello"]);

        worker.exit();
    }

    #[tokio::test]
    async fn start_stop_event_loop() {
        let output_stream = OutputStream::new();

        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(32);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send_many([
            WorkerMessage::Update {
                graph: graph.clone(),
                func_lib: Arc::new(func_lib.clone()),
            },
            WorkerMessage::StartEventLoop,
        ]);

        // Wait for event loop to start and produce some output
        let _ = compute_finish_rx.recv().await; // Initial execution
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(worker.is_event_loop_started());

        // Stop the event loop
        worker.send(WorkerMessage::StopEventLoop);
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(!worker.is_event_loop_started());

        worker.exit();
    }

    #[tokio::test]
    async fn request_argument_values_invokes_callback() {
        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::default();

        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send_many([
            WorkerMessage::Update {
                graph: graph.clone(),
                func_lib: Arc::new(func_lib.clone()),
            },
            WorkerMessage::Event {
                event: EventRef {
                    node_id: frame_event_node_id,
                    event_idx: 0,
                },
            },
        ]);

        // Wait for execution
        let _ = compute_finish_rx.recv().await;

        let (reply, rx) = oneshot::channel();
        worker.send(WorkerMessage::RequestArgumentValues {
            node_id: frame_event_node_id,
            reply,
        });

        let values = timeout(Duration::from_millis(200), rx)
            .await
            .expect("Reply timeout")
            .expect("Reply sender dropped");

        assert!(values.is_some());

        worker.exit();
    }

    #[tokio::test]
    async fn ack_fires_after_execution() {
        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::default();

        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        let (reply, rx) = oneshot::channel();
        worker.send_many([
            WorkerMessage::Update {
                graph: graph.clone(),
                func_lib: Arc::new(func_lib.clone()),
            },
            WorkerMessage::Event {
                event: EventRef {
                    node_id: frame_event_node_id,
                    event_idx: 0,
                },
            },
            WorkerMessage::Ack { reply },
        ]);

        let _ = compute_finish_rx.recv().await;
        timeout(Duration::from_millis(200), rx)
            .await
            .expect("Ack timeout")
            .expect("Ack sender dropped");

        worker.exit();
    }

    #[tokio::test]
    async fn update_restarts_event_loop_if_running() {
        let output_stream = OutputStream::new();

        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(32);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        // Start with event loop
        worker.send_many([
            WorkerMessage::Update {
                graph: graph.clone(),
                func_lib: Arc::new(func_lib.clone()),
            },
            WorkerMessage::StartEventLoop,
        ]);

        let _ = compute_finish_rx.recv().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(worker.is_event_loop_started());

        // Update graph - should restart event loop
        worker.send(WorkerMessage::Update {
            graph: graph.clone(),
            func_lib: Arc::new(func_lib.clone()),
        });

        // Drain the channel
        while compute_finish_rx.try_recv().is_ok() {}

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Event loop should still be running after update
        assert!(worker.is_event_loop_started());

        worker.exit();
    }

    // Stale-event filtering is now structural: each start_event_loop
    // call returns a fresh Receiver; reset_event_loop drops the old
    // pair so any undelivered events die with the channel. This test
    // verifies the structural guarantee by confirming the old
    // Receiver is closed after its sibling handle is stopped.
    #[tokio::test]
    async fn stopped_event_loop_channel_is_closed() {
        let node_id = NodeId::unique();
        let event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
        let event_state = SharedAnyState::default();

        let (mut handle, mut event_rx) = super::start_event_loop(
            vec![EventTrigger {
                event: EventRef {
                    node_id,
                    event_idx: 0,
                },
                lambda: event_lambda,
                state: event_state,
            }],
            PauseGate::default(),
        )
        .await;

        handle.stop().await;

        // After stop, all lambda tasks (the sole senders) are aborted
        // → the Receiver must observe channel closure on next recv().
        let closed = timeout(Duration::from_millis(500), event_rx.recv())
            .await
            .expect("recv should complete once channel closes");
        // recv_many / recv return None when all senders are dropped.
        // Buffered events may still be present (fine); what matters
        // is that eventually we get None — drain and confirm.
        if closed.is_some() {
            while event_rx.recv().await.is_some() {}
        }
    }

    #[tokio::test]
    async fn send_many_empty_is_noop() {
        // Empty batch must not panic, hang, or desynchronize the worker.
        let mut worker = Worker::new(|_| {});

        worker.send_many(std::iter::empty::<WorkerMessage>());

        // Subsequent Ack still fires → worker is alive.
        let (reply, rx) = oneshot::channel();
        worker.send(WorkerMessage::Ack { reply });
        timeout(Duration::from_millis(500), rx)
            .await
            .expect("Ack should fire after empty send_many")
            .expect("Ack sender dropped");

        worker.exit();
    }

    #[tokio::test]
    async fn stop_event_loop_when_not_running_is_noop() {
        let mut worker = Worker::new(|_| {});

        worker.send(WorkerMessage::StopEventLoop);
        assert!(!worker.is_event_loop_started());

        // Worker still responsive after a no-op stop.
        let (reply, rx) = oneshot::channel();
        worker.send(WorkerMessage::Ack { reply });
        timeout(Duration::from_millis(500), rx)
            .await
            .expect("StopEventLoop with no running loop should be a no-op")
            .expect("Ack sender dropped");

        worker.exit();
    }

    #[tokio::test]
    async fn request_argument_values_for_unknown_node_returns_none() {
        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::default();
        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);

        let (compute_finish_tx, _compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send(WorkerMessage::Update {
            graph,
            func_lib: Arc::new(func_lib),
        });

        let (reply, rx) = oneshot::channel();
        worker.send(WorkerMessage::RequestArgumentValues {
            node_id: NodeId::unique(),
            reply,
        });

        let values = timeout(Duration::from_millis(500), rx)
            .await
            .expect("Reply timeout")
            .expect("Reply sender dropped");

        assert!(values.is_none(), "unknown node should yield None");

        worker.exit();
    }

    #[tokio::test]
    async fn multiple_acks_in_batch_all_run() {
        let mut worker = Worker::new(|_| {});

        let (reply_a, rx_a) = oneshot::channel();
        let (reply_b, rx_b) = oneshot::channel();

        worker.send_many([
            WorkerMessage::Ack { reply: reply_a },
            WorkerMessage::Ack { reply: reply_b },
        ]);

        timeout(Duration::from_millis(500), rx_a)
            .await
            .expect("First Ack should fire")
            .expect("First sender dropped");
        timeout(Duration::from_millis(500), rx_b)
            .await
            .expect("Second Ack should fire")
            .expect("Second sender dropped");

        worker.exit();
    }

    #[tokio::test]
    async fn clear_then_update_in_same_batch_applies_update() {
        // Scan-then-commit ordering: Clear zeroes execution_graph, Update
        // queues a replacement, commit phase applies Update and flips
        // execution_graph_clear back to false. The event must execute.
        let output_stream = OutputStream::new();
        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;
        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send_many([
            WorkerMessage::Clear,
            WorkerMessage::Update {
                graph,
                func_lib: Arc::new(func_lib),
            },
            WorkerMessage::Event {
                event: EventRef {
                    node_id: frame_event_node_id,
                    event_idx: 0,
                },
            },
        ]);

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes.len(), 3);
        assert_eq!(output_stream.take().await, ["1"]);

        worker.exit();
    }

    // F3: RequestArgumentValues batched with Update must observe the
    // post-Update graph. Before the fix this ran inline during the scan
    // phase and returned `None` because the Update was still pending in
    // `BatchIntent`; after the fix it runs in commit phase and returns
    // `Some`.
    #[tokio::test]
    async fn request_argument_values_batched_with_update_sees_new_graph() {
        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::default();
        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        // Worker starts with an empty ExecutionGraph. Batch Update +
        // RequestArgumentValues together: only works if the scan phase
        // defers the request until after Update commits.
        let (reply, rx) = oneshot::channel();
        worker.send_many([
            WorkerMessage::Update {
                graph,
                func_lib: Arc::new(func_lib),
            },
            WorkerMessage::RequestArgumentValues {
                node_id: frame_event_node_id,
                reply,
            },
        ]);

        let values = timeout(Duration::from_millis(500), rx)
            .await
            .expect("Reply timeout")
            .expect("Reply sender dropped");

        assert!(
            values.is_some(),
            "request batched with Update must see the newly-loaded graph"
        );

        worker.exit();
    }

    // F3: Clear batched after an Update+Execute must clear the graph
    // before a same-batch RequestArgumentValues runs → request returns
    // None.
    #[tokio::test]
    async fn request_argument_values_batched_with_clear_sees_empty_graph() {
        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::default();
        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        // First populate the graph.
        worker.send(WorkerMessage::Update {
            graph,
            func_lib: Arc::new(func_lib),
        });
        // Sanity: prior request sees values.
        let (reply_before, rx_before) = oneshot::channel();
        worker.send(WorkerMessage::RequestArgumentValues {
            node_id: frame_event_node_id,
            reply: reply_before,
        });
        assert!(
            timeout(Duration::from_millis(500), rx_before)
                .await
                .expect("pre-clear reply timeout")
                .expect("pre-clear sender dropped")
                .is_some()
        );

        // Now batch Clear + RequestArgumentValues. Request must see the
        // post-clear state (None), not the pre-clear state (Some).
        let (reply_after, rx_after) = oneshot::channel();
        worker.send_many([
            WorkerMessage::Clear,
            WorkerMessage::RequestArgumentValues {
                node_id: frame_event_node_id,
                reply: reply_after,
            },
        ]);
        let values = timeout(Duration::from_millis(500), rx_after)
            .await
            .expect("post-clear reply timeout")
            .expect("post-clear sender dropped");
        assert!(
            values.is_none(),
            "request batched after Clear must observe empty graph"
        );

        // Drain any execution-finished callbacks the test produced.
        while compute_finish_rx.try_recv().is_ok() {}

        worker.exit();
    }

    // F4: Firing events or ExecuteTerminals against an empty graph
    // must produce exactly one callback with `Err(EmptyGraph)` — not
    // silent log-and-drop.
    #[tokio::test]
    async fn execute_terminals_on_empty_graph_reports_empty_graph_error() {
        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send(WorkerMessage::ExecuteTerminals);

        let result = timeout(Duration::from_millis(500), compute_finish_rx.recv())
            .await
            .expect("ExecuteTerminals on empty graph must fire callback")
            .expect("callback channel closed");

        assert!(
            matches!(result, Err(Error::EmptyGraph)),
            "empty-graph execution must yield Err(EmptyGraph), got {result:?}"
        );

        worker.exit();
    }

    #[tokio::test]
    async fn event_on_empty_graph_reports_empty_graph_error() {
        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send(WorkerMessage::Event {
            event: EventRef {
                node_id: NodeId::unique(),
                event_idx: 0,
            },
        });

        let result = timeout(Duration::from_millis(500), compute_finish_rx.recv())
            .await
            .expect("Event on empty graph must fire callback")
            .expect("callback channel closed");

        assert!(
            matches!(result, Err(Error::EmptyGraph)),
            "empty-graph event must yield Err(EmptyGraph), got {result:?}"
        );

        worker.exit();
    }

    #[tokio::test]
    async fn start_event_loop_on_empty_graph_reports_empty_graph_error() {
        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send(WorkerMessage::StartEventLoop);

        let result = timeout(Duration::from_millis(500), compute_finish_rx.recv())
            .await
            .expect("StartEventLoop on empty graph must fire callback")
            .expect("callback channel closed");

        assert!(
            matches!(result, Err(Error::EmptyGraph)),
            "empty-graph start-loop must yield Err(EmptyGraph), got {result:?}"
        );
        assert!(
            !worker.is_event_loop_started(),
            "no loop should actually have started"
        );

        worker.exit();
    }

    // --- Pure scan() unit tests ------------------------------------

    #[test]
    fn scan_accumulates_simple_flags() {
        let (reply_ack, _ack_rx) = oneshot::channel();
        let (reply_args, _args_rx) = oneshot::channel();
        let node_id = NodeId::unique();
        let event = EventRef {
            node_id,
            event_idx: 0,
        };

        let intent = super::scan(vec![
            WorkerMessage::Clear,
            WorkerMessage::StartEventLoop,
            WorkerMessage::ExecuteTerminals,
            WorkerMessage::Event { event },
            WorkerMessage::Ack { reply: reply_ack },
            WorkerMessage::RequestArgumentValues {
                node_id,
                reply: reply_args,
            },
        ]);

        assert!(matches!(intent.graph_state, Some(super::GraphOp::Clear)));
        assert!(matches!(
            intent.loop_request,
            Some(super::LoopCommand::Start)
        ));
        assert!(intent.execute_terminals);
        assert!(!intent.exit);
        assert_eq!(intent.events.len(), 1);
        assert!(intent.events.contains(&event));
        assert_eq!(intent.acks.len(), 1);
        assert_eq!(intent.argument_requests.len(), 1);
        assert_eq!(intent.argument_requests[0].0, node_id);
    }

    #[test]
    fn scan_deduplicates_events() {
        let node_id = NodeId::unique();
        let event = EventRef {
            node_id,
            event_idx: 0,
        };

        let intent = super::scan(vec![
            WorkerMessage::Event { event },
            WorkerMessage::Event { event },
            WorkerMessage::Events {
                events: vec![event, event],
            },
        ]);

        assert_eq!(
            intent.events.len(),
            1,
            "duplicate events must collapse to one"
        );
    }

    #[test]
    fn scan_exit_dominates_entire_batch() {
        // Exit is sticky across the whole batch: every other command
        // in the batch is discarded, whether sent before or after.
        let intent = super::scan(vec![
            WorkerMessage::Clear,
            WorkerMessage::ExecuteTerminals,
            WorkerMessage::Exit,
            WorkerMessage::StartEventLoop, // post-Exit: dropped
            WorkerMessage::Update {
                graph: Graph::default(),
                func_lib: Arc::new(FuncLib::default()),
            },
        ]);

        assert!(intent.exit);
        assert!(
            intent.graph_state.is_none(),
            "pre-Exit graph ops must be discarded"
        );
        assert!(
            intent.loop_request.is_none(),
            "post-Exit loop ops must be discarded"
        );
        assert!(
            !intent.execute_terminals,
            "pre-Exit execute_terminals must be discarded"
        );
        assert!(intent.events.is_empty());
        assert!(intent.acks.is_empty());
        assert!(intent.argument_requests.is_empty());
    }

    #[test]
    fn scan_update_overwrites_earlier_update_in_same_batch() {
        // Two Updates in one batch: the last one wins. This is
        // implicit today (Option::replace) but worth pinning since
        // callers do send [Update(A), Update(B)] during rapid edits.
        let empty_graph = Graph::default();
        let func_lib = Arc::new(FuncLib::default());

        let intent = super::scan(vec![
            WorkerMessage::Update {
                graph: empty_graph.clone(),
                func_lib: func_lib.clone(),
            },
            WorkerMessage::Update {
                graph: empty_graph.clone(),
                func_lib: func_lib.clone(),
            },
        ]);

        assert!(matches!(
            intent.graph_state,
            Some(super::GraphOp::Replace(_, _))
        ));
    }

    // Slot reduction: last-write-wins for graph_state and loop_request.

    #[test]
    fn scan_clear_then_update_yields_replace() {
        let intent = super::scan(vec![
            WorkerMessage::Clear,
            WorkerMessage::Update {
                graph: Graph::default(),
                func_lib: Arc::new(FuncLib::default()),
            },
        ]);
        assert!(
            matches!(intent.graph_state, Some(super::GraphOp::Replace(_, _))),
            "last write (Update) wins over earlier Clear"
        );
    }

    #[test]
    fn scan_update_then_clear_yields_clear() {
        let intent = super::scan(vec![
            WorkerMessage::Update {
                graph: Graph::default(),
                func_lib: Arc::new(FuncLib::default()),
            },
            WorkerMessage::Clear,
        ]);
        assert!(
            matches!(intent.graph_state, Some(super::GraphOp::Clear)),
            "last write (Clear) wins over earlier Update"
        );
    }

    #[test]
    fn scan_start_then_stop_yields_stop() {
        let intent = super::scan(vec![
            WorkerMessage::StartEventLoop,
            WorkerMessage::StopEventLoop,
        ]);
        assert!(
            matches!(intent.loop_request, Some(super::LoopCommand::Stop)),
            "last write (Stop) wins over earlier Start"
        );
    }

    #[test]
    fn scan_stop_then_start_yields_start() {
        let intent = super::scan(vec![
            WorkerMessage::StopEventLoop,
            WorkerMessage::StartEventLoop,
        ]);
        assert!(
            matches!(intent.loop_request, Some(super::LoopCommand::Start)),
            "last write (Start) wins over earlier Stop"
        );
    }

    // Integration: end-to-end confirmation that Update-then-Clear in
    // one batch leaves the execution graph empty — a subsequent Event
    // sees the empty graph and yields Err(EmptyGraph).
    #[tokio::test]
    async fn update_then_clear_in_same_batch_leaves_graph_cleared() {
        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::default();
        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send_many([
            WorkerMessage::Update {
                graph,
                func_lib: Arc::new(func_lib),
            },
            WorkerMessage::Clear,
            WorkerMessage::Event {
                event: EventRef {
                    node_id: frame_event_node_id,
                    event_idx: 0,
                },
            },
        ]);

        let result = timeout(Duration::from_millis(500), compute_finish_rx.recv())
            .await
            .expect("event on post-Clear graph must fire callback")
            .expect("callback channel closed");
        assert!(
            matches!(result, Err(Error::EmptyGraph)),
            "Update followed by Clear must leave the graph empty, got {result:?}"
        );

        worker.exit();
    }

    // --- `biased` select: commands not starved by events -----------

    // Lambda fires as fast as it can; a Stop command must be observed
    // and acted on within a bounded time rather than being delayed
    // indefinitely by the event stream. Without `biased;` on the
    // select!, this test would flake — the fair-random polling could
    // sit on the events branch for many iterations.
    #[tokio::test]
    async fn commands_not_starved_by_fast_event_loop() {
        let output_stream = OutputStream::new();
        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;
        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(512);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send_many([
            WorkerMessage::Update {
                graph,
                func_lib: Arc::new(func_lib),
            },
            WorkerMessage::StartEventLoop,
        ]);

        // Give the loop time to build up momentum.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(worker.is_event_loop_started());

        // Drain accumulated callbacks so the channel isn't a
        // confounding factor.
        while compute_finish_rx.try_recv().is_ok() {}

        // Send Stop + Ack. Both must be observed within the budget
        // even though lambda events are still being produced.
        let (reply, rx) = oneshot::channel();
        worker.send_many([WorkerMessage::StopEventLoop, WorkerMessage::Ack { reply }]);

        timeout(Duration::from_millis(500), rx)
            .await
            .expect("Ack after StopEventLoop must fire promptly despite event load")
            .expect("Ack sender dropped");

        assert!(
            !worker.is_event_loop_started(),
            "event loop should be stopped"
        );

        worker.exit();
    }

    // End-to-end: an event fired by a lambda reaches the worker's
    // execute path and produces an execution_callback. Covers the
    // dedicated bounded-channel flow (no forwarder).
    #[tokio::test]
    async fn lambda_events_drive_worker_execution() {
        let output_stream = OutputStream::new();
        let timers_invoker = WorkerEventsFuncLib::default();
        let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;
        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(32);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        worker.send_many([
            WorkerMessage::Update {
                graph,
                func_lib: Arc::new(func_lib),
            },
            WorkerMessage::StartEventLoop,
        ]);

        // First callback is the initial execute() inside the
        // start_event_loop path.
        let _ = timeout(Duration::from_millis(500), compute_finish_rx.recv())
            .await
            .expect("initial execute callback")
            .expect("callback channel closed");

        // Subsequent callbacks must come from the lambda firing and
        // the worker draining events from the bounded channel.
        let second = timeout(Duration::from_millis(500), compute_finish_rx.recv())
            .await
            .expect("lambda-driven execute callback")
            .expect("callback channel closed");
        assert!(
            second.is_ok(),
            "lambda-driven execution must succeed: {second:?}"
        );

        worker.exit();
    }
}
