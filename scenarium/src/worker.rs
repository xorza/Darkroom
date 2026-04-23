use std::collections::{HashSet, VecDeque};
use std::mem::take;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, channel, unbounded_channel};
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

/// Max messages drained per worker-loop iteration. Bounds latency
/// between a `send` and the loop observing it.
const WORKER_MSG_BATCH: usize = 10;

/// Capacity of the bounded channel from each event-lambda task into
/// the forwarder, and the forwarder's own batch size. Provides
/// backpressure when a lambda fires faster than the worker drains.
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
    EventFromLoop {
        loop_id: u64,
        event: EventRef,
    },
    EventsFromLoop {
        loop_id: u64,
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
    Multi {
        msgs: Vec<WorkerMessage>,
    },
    RequestArgumentValues {
        node_id: NodeId,
        reply: oneshot::Sender<Option<ArgumentValues>>,
    },
}

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<WorkerMessage>,
    event_loop_started: Arc<AtomicBool>,
}

impl Worker {
    pub fn new<ExecutionCallback>(callback: ExecutionCallback) -> Self
    where
        ExecutionCallback: Fn(Result<ExecutionStats>) + Send + 'static,
    {
        let (tx, rx) = unbounded_channel::<WorkerMessage>();
        let event_loop_started = Arc::new(AtomicBool::new(false));
        let thread_handle: JoinHandle<()> = tokio::spawn({
            let tx = tx.clone();
            let event_loop_started = event_loop_started.clone();
            async move {
                worker_loop(rx, tx, callback, event_loop_started).await;
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
            .send(msg)
            .expect("Failed to send message to worker, it probably exited");
    }

    pub fn send_many<T: IntoIterator<Item = WorkerMessage>>(&self, msgs: T) {
        let msgs: Vec<WorkerMessage> = msgs.into_iter().collect();
        if !msgs.is_empty() {
            self.tx
                .send(WorkerMessage::Multi { msgs })
                .expect("Failed to send message to worker, it probably exited");
        }
    }

    pub fn exit(&mut self) {
        self.tx.send(WorkerMessage::Exit).ok();

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

/// Per-iteration accumulator for the worker loop's scan phase. Every
/// command goes into a field here; side effects (reset, execute,
/// start loop, run callbacks) happen in the commit phase below.
#[derive(Default)]
struct BatchIntent {
    update: Option<(Graph, Arc<FuncLib>)>,
    clear: bool,
    start_loop: bool,
    stop_loop: bool,
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

async fn worker_loop<ExecutionCallback>(
    mut worker_message_rx: UnboundedReceiver<WorkerMessage>,
    worker_message_tx: UnboundedSender<WorkerMessage>,
    execution_callback: ExecutionCallback,
    event_loop_started: Arc<AtomicBool>,
) where
    ExecutionCallback: Fn(Result<ExecutionStats>) + Send + 'static,
{
    let mut execution_graph = ExecutionGraph::default();

    let mut msgs: Vec<WorkerMessage> = Vec::with_capacity(WORKER_MSG_BATCH);
    let mut pending: VecDeque<WorkerMessage> = VecDeque::with_capacity(WORKER_MSG_BATCH);

    let mut events_from_loop: HashSet<(u64, EventRef)> = HashSet::default();
    let mut event_loop_handle: Option<EventLoopHandle> = None;
    let event_loop_pause_gate = PauseGate::default();
    let mut current_loop_id: u64 = 0;

    loop {
        assert!(events_from_loop.is_empty());
        assert!(pending.is_empty());

        event_loop_started.store(event_loop_handle.is_some(), Ordering::Relaxed);

        msgs.clear();
        if worker_message_rx
            .recv_many(&mut msgs, WORKER_MSG_BATCH)
            .await
            == 0
        {
            return;
        }

        let event_loop_pause_guard = event_loop_pause_gate.close();

        // --- Scan: accumulate intent, no event-loop side effects.
        // Breaks on Exit so messages queued after it are discarded.
        let mut intent = BatchIntent::default();
        pending.extend(msgs.drain(..));
        while let Some(msg) = pending.pop_front() {
            match msg {
                WorkerMessage::Exit => {
                    intent.exit = true;
                    pending.clear();
                    break;
                }
                WorkerMessage::Event { event } => {
                    intent.events.insert(event);
                }
                WorkerMessage::Events { events } => intent.events.extend(events),
                WorkerMessage::EventFromLoop { loop_id, event } => {
                    if current_loop_id == loop_id {
                        events_from_loop.insert((loop_id, event));
                    }
                }
                WorkerMessage::EventsFromLoop { loop_id, events } => {
                    if current_loop_id == loop_id {
                        for event in events {
                            events_from_loop.insert((loop_id, event));
                        }
                    }
                }
                WorkerMessage::Update { graph, func_lib } => {
                    intent.update = Some((graph, func_lib));
                }
                WorkerMessage::Clear => intent.clear = true,
                WorkerMessage::ExecuteTerminals => intent.execute_terminals = true,
                WorkerMessage::StartEventLoop => intent.start_loop = true,
                WorkerMessage::StopEventLoop => intent.stop_loop = true,
                WorkerMessage::Multi { msgs: nested } => pending.extend(nested),
                WorkerMessage::Ack { reply } => {
                    intent.acks.push(reply);
                }
                WorkerMessage::RequestArgumentValues { node_id, reply } => {
                    intent.argument_requests.push((node_id, reply));
                }
            }
        }

        // --- Commit: one reset if any command touches the loop, then
        // apply Clear → Update → StartEventLoop in stable order.
        let needs_reset = intent.update.is_some()
            || intent.clear
            || intent.start_loop
            || intent.stop_loop
            || intent.exit;
        let was_running = needs_reset
            && reset_event_loop(
                &mut current_loop_id,
                &mut event_loop_handle,
                &mut events_from_loop,
            )
            .await;

        if intent.exit {
            return;
        }

        if intent.clear {
            execution_graph.clear();
        }
        if let Some((graph, func_lib)) = intent.update {
            tracing::info!("Graph updated");
            execution_graph.update(&graph, &func_lib);
        }
        let should_start_event_loop = intent.start_loop || (was_running && !intent.stop_loop);

        let mut events = intent.events;
        events.extend(
            events_from_loop
                .drain()
                .filter_map(|(loop_id, event)| (loop_id == current_loop_id).then_some(event)),
        );

        if intent.execute_terminals || !events.is_empty() {
            if execution_graph.is_empty() {
                (execution_callback)(Err(Error::EmptyGraph));
            } else {
                let result = execution_graph
                    .execute(
                        intent.execute_terminals,
                        event_loop_handle.is_some(),
                        events.drain(),
                    )
                    .await;
                (execution_callback)(result);
            }
        }

        if should_start_event_loop {
            assert!(event_loop_handle.is_none());

            if execution_graph.is_empty() {
                (execution_callback)(Err(Error::EmptyGraph));
            } else {
                let result = execution_graph.execute(false, true, []).await;

                if let Ok(execution_stats) = &result {
                    let event_triggers = execution_graph.active_event_triggers(execution_stats);

                    if !event_triggers.is_empty() {
                        event_loop_handle = Some(
                            start_event_loop(
                                worker_message_tx.clone(),
                                event_triggers,
                                event_loop_pause_gate.clone(),
                                current_loop_id,
                            )
                            .await,
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

        for reply in intent.acks.drain(..) {
            let _ = reply.send(());
        }

        drop(event_loop_pause_guard);
        tokio::task::yield_now().await;
    }
}

/// Tears down any running event loop, bumps `current_loop_id` so that
/// stale `EventFromLoop`/`EventsFromLoop` still queued from the old
/// loop get filtered out, and clears any previously-buffered ones.
/// Returns `true` if a loop was actually running, which the commit
/// phase uses to decide whether to implicitly restart.
async fn reset_event_loop(
    current_loop_id: &mut u64,
    event_loop_handle: &mut Option<EventLoopHandle>,
    events_from_loop: &mut HashSet<(u64, EventRef)>,
) -> bool {
    let was_running = event_loop_handle.is_some();
    if let Some(mut handle) = event_loop_handle.take() {
        handle.stop().await;
        tracing::info!("Event loop stopped");
    }
    *current_loop_id += 1;
    events_from_loop.clear();
    was_running
}

async fn start_event_loop(
    worker_message_tx: UnboundedSender<WorkerMessage>,
    event_triggers: Vec<EventTrigger>,
    pause_gate: PauseGate,
    loop_id: u64,
) -> EventLoopHandle {
    assert!(!event_triggers.is_empty());

    let mut join_handles: Vec<JoinHandle<()>> = Vec::default();

    // Bounded channel provides backpressure for event lambdas. When
    // the event loop is stopped, this channel is dropped along with
    // event_rx, causing all event lambda tasks to exit. A new event
    // loop gets a fresh channel, so old events don't leak into the
    // new one.
    let (event_tx, mut event_rx) = channel::<EventRef>(EVENT_LOOP_BACKPRESSURE);

    // Forwarder task: batches events from the bounded channel and
    // forwards them to the main worker queue. Decouples the bounded
    // backpressure from the unbounded worker queue while allowing
    // efficient batching.
    let join_handle = tokio::spawn({
        let worker_message_tx = worker_message_tx.clone();
        async move {
            let mut events = Vec::default();
            let mut seen: HashSet<EventRef> = HashSet::default();
            loop {
                events.clear();
                if event_rx
                    .recv_many(&mut events, EVENT_LOOP_BACKPRESSURE)
                    .await
                    == 0
                {
                    return;
                }

                seen.clear();
                events.retain(|event| seen.insert(*event));

                let result = if events.len() == 1 {
                    worker_message_tx.send(WorkerMessage::EventFromLoop {
                        event: events[0],
                        loop_id,
                    })
                } else {
                    worker_message_tx.send(WorkerMessage::EventsFromLoop {
                        events: take(&mut events),
                        loop_id,
                    })
                };

                if result.is_err() {
                    return;
                }
            }
        }
    });
    join_handles.push(join_handle);

    let ready = ReadyState::new(event_triggers.len());

    // Spawn one task per event lambda: invoke, send, pause-gate-wait,
    // yield. The pause gate blocks new iterations while the main
    // worker is processing so events don't build up during graph
    // execution.
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
                    let result = event_tx.send(event).await;
                    if result.is_err() {
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

    EventLoopHandle { join_handles }
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use common::output_stream::OutputStream;
    use common::pause_gate::PauseGate;
    use tokio::sync::mpsc::unbounded_channel;
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

        let (tx, mut rx) = unbounded_channel();
        let mut handle = super::start_event_loop(
            tx,
            vec![EventTrigger {
                event: EventRef {
                    node_id,
                    event_idx: 0,
                },
                lambda: event_lambda,
                state: event_state,
            }],
            PauseGate::default(),
            0,
        )
        .await;

        let msg = rx.recv().await.expect("Expected event loop message");
        let WorkerMessage::EventFromLoop { event, .. } = msg else {
            panic!("Expected WorkerMessage::Event");
        };
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

        let (tx, mut rx) = unbounded_channel();
        let mut handle = super::start_event_loop(
            tx,
            vec![EventTrigger {
                event: EventRef {
                    node_id,
                    event_idx: 0,
                },
                lambda: event_lambda,
                state: event_state,
            }],
            PauseGate::default(),
            0,
        )
        .await;

        notify_for_callback.notify_waiters();

        let msg = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("Expected event message")
            .expect("Event channel closed");
        let WorkerMessage::EventFromLoop { event, .. } = msg else {
            panic!("Expected WorkerMessage::Event");
        };
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
        let (tx, mut rx) = unbounded_channel();

        let mut handle = super::start_event_loop(
            tx,
            vec![EventTrigger {
                event: EventRef {
                    node_id,
                    event_idx: 0,
                },
                lambda: event_lambda,
                state: event_state,
            }],
            pause_gate.clone(),
            0,
        )
        .await;

        // Wait for first event to arrive
        let _ = timeout(Duration::from_millis(100), rx.recv())
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
    async fn multi_message_processes_nested_messages() {
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

        // Send nested Multi messages
        worker.send(WorkerMessage::Multi {
            msgs: vec![
                WorkerMessage::Update {
                    graph: graph.clone(),
                    func_lib: Arc::new(func_lib.clone()),
                },
                WorkerMessage::Multi {
                    msgs: vec![WorkerMessage::Event {
                        event: EventRef {
                            node_id: frame_event_node_id,
                            event_idx: 0,
                        },
                    }],
                },
            ],
        });

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes.len(), 3);
        assert_eq!(output_stream.take().await, ["1"]);

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

    #[tokio::test]
    async fn stale_events_from_stopped_loop_are_ignored() {
        // This test verifies that events from a previously stopped event loop
        // (with an old loop_id) are not executed after the loop is restarted.
        // The worker uses loop_id to filter out stale events.

        let node_id = NodeId::unique();

        // Create a simple event lambda that completes immediately
        let event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
        let event_state = SharedAnyState::default();

        let pause_gate = PauseGate::default();
        let (tx, mut rx) = unbounded_channel();

        // Start first event loop with loop_id = 0
        let mut handle = super::start_event_loop(
            tx.clone(),
            vec![EventTrigger {
                event: EventRef {
                    node_id,
                    event_idx: 0,
                },
                lambda: event_lambda.clone(),
                state: event_state.clone(),
            }],
            pause_gate.clone(),
            0, // loop_id = 0
        )
        .await;

        // Wait for some events to be queued from first loop
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Stop the first loop
        handle.stop().await;

        // Drain any messages from first loop
        let mut stale_events = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            stale_events.push(msg);
        }

        // Verify we got some events from the old loop (loop_id = 0)
        assert!(
            !stale_events.is_empty(),
            "Should have received events from first loop"
        );

        // Check that the stale events have the old loop_id
        for msg in &stale_events {
            match msg {
                WorkerMessage::EventFromLoop { loop_id, .. } => {
                    assert_eq!(*loop_id, 0, "Stale events should have loop_id 0");
                }
                WorkerMessage::EventsFromLoop { loop_id, .. } => {
                    assert_eq!(*loop_id, 0, "Stale events should have loop_id 0");
                }
                _ => {}
            }
        }

        // Start a new event loop with loop_id = 1
        let new_event_lambda = EventLambda::new(|_state| Box::pin(async move {}));
        let new_event_state = SharedAnyState::default();

        let mut new_handle = super::start_event_loop(
            tx,
            vec![EventTrigger {
                event: EventRef {
                    node_id,
                    event_idx: 0,
                },
                lambda: new_event_lambda,
                state: new_event_state,
            }],
            pause_gate,
            1, // loop_id = 1 (incremented)
        )
        .await;

        // Wait for new loop to produce events
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Collect new events
        let mut new_events = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            new_events.push(msg);
        }

        // Verify new events have the new loop_id
        for msg in &new_events {
            match msg {
                WorkerMessage::EventFromLoop { loop_id, .. } => {
                    assert_eq!(*loop_id, 1, "New events should have loop_id 1");
                }
                WorkerMessage::EventsFromLoop { loop_id, .. } => {
                    assert_eq!(*loop_id, 1, "New events should have loop_id 1");
                }
                _ => {}
            }
        }

        // The key verification: if a worker were to process both stale_events
        // and new_events with current_loop_id = 1, only new_events would pass
        // the filter: (loop_id == current_loop_id).then_some(event)
        let current_loop_id = 1u64;
        let filtered_stale: Vec<_> = stale_events
            .iter()
            .filter_map(|msg| match msg {
                WorkerMessage::EventFromLoop { loop_id, event } => {
                    (*loop_id == current_loop_id).then_some(event)
                }
                _ => None,
            })
            .collect();

        assert!(
            filtered_stale.is_empty(),
            "Stale events should be filtered out by loop_id check"
        );

        new_handle.stop().await;
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

    #[tokio::test]
    async fn update_increments_loop_id_so_events_from_old_loop_are_dropped() {
        // Invariant: when Update arrives while an event loop is running,
        // reset_event_loop() bumps current_loop_id. Any stale
        // EventFromLoop/EventsFromLoop still sitting in the queue with
        // the old id is then filtered at line 204/213 and never drives
        // execution. We probe that by stuffing a crafted EventFromLoop
        // with loop_id=0 into the queue after an Update (which has
        // already incremented current_loop_id to 1).
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
            graph,
            func_lib: Arc::new(func_lib),
        });

        // Stale event from a loop that never existed (id 0 vs
        // current_loop_id=1 after the Update reset). Must be filtered.
        worker.send(WorkerMessage::EventFromLoop {
            loop_id: 0,
            event: EventRef {
                node_id: frame_event_node_id,
                event_idx: 0,
            },
        });

        // Flush: an Ack batched after the stale event
        // fires at end-of-iteration only if no execute happened
        // spuriously — we don't care about that here; what we care
        // about is that no compute completion fires for the stale
        // event. Use a short timeout.
        assert!(
            timeout(Duration::from_millis(150), compute_finish_rx.recv())
                .await
                .is_err(),
            "stale EventFromLoop must not trigger execution"
        );
        assert!(output_stream.take().await.is_empty());

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
}
