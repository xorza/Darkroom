use crate::event::EventLambda;
use crate::event_state::EventState;
use crate::execution_graph::{ArgumentValues, ExecutionGraph, ExecutionStats, Result};
use crate::function::FuncLib;
use crate::graph::{Graph, NodeId};
use common::ReadyState;
use common::pause_gate::PauseGate;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::mem::take;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, channel, unbounded_channel};
use tokio::task::JoinHandle;

const MAX_EVENTS_PER_LOOP: usize = 10;

#[derive(Debug, Default)]
pub enum WorkerMessage {
    #[default]
    Noop,
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
        func_lib: FuncLib,
    },
    Clear,
    ExecuteTerminals,
    StartEventLoop,
    StopEventLoop,
    ProcessingCallback {
        callback: ProcessingCallback,
    },
    Multi {
        msgs: Vec<WorkerMessage>,
    },
    RequestArgumentValues {
        node_id: NodeId,
        callback: ArgumentValuesCallback,
    },
}

pub struct ProcessingCallback {
    inner: Box<dyn FnOnce() + Send + Sync + 'static>,
}

pub struct ArgumentValuesCallback {
    inner: Box<dyn FnOnce(Option<ArgumentValues>) + Send + Sync + 'static>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventRef {
    pub node_id: NodeId,
    pub event_idx: usize,
}

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<WorkerMessage>,
    event_loop_started: Arc<AtomicBool>,
}

#[derive(Debug)]
struct EventLoopHandle {
    join_handles: Vec<JoinHandle<()>>,
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

async fn worker_loop<ExecutionCallback>(
    mut worker_message_rx: UnboundedReceiver<WorkerMessage>,
    worker_message_tx: UnboundedSender<WorkerMessage>,
    execution_callback: ExecutionCallback,
    event_loop_started: Arc<AtomicBool>,
) where
    ExecutionCallback: Fn(Result<ExecutionStats>) + Send + 'static,
{
    let mut execution_graph = ExecutionGraph::default();

    let mut msgs: Vec<WorkerMessage> = Vec::with_capacity(MAX_EVENTS_PER_LOOP);

    let mut events: HashSet<EventRef> = HashSet::default();
    let mut events_from_loop: HashSet<(u64, EventRef)> = HashSet::default();
    let mut event_loop_handle: Option<EventLoopHandle> = None;
    let mut processing_callback: Option<ProcessingCallback> = None;
    let event_loop_pause_gate = PauseGate::default();
    let mut current_loop_id: u64 = 0;
    let mut execution_graph_clear = true;

    loop {
        assert!(events.is_empty());
        assert!(events_from_loop.is_empty());
        assert!(processing_callback.is_none());

        event_loop_started.store(event_loop_handle.is_some(), Ordering::Relaxed);

        msgs.clear();
        if worker_message_rx
            .recv_many(&mut msgs, MAX_EVENTS_PER_LOOP)
            .await
            == 0
        {
            return;
        }

        let event_loop_pause_guard = event_loop_pause_gate.close();

        let mut execute_terminals: bool = false;
        let mut update_graph: Option<(Graph, FuncLib)> = None;
        let mut should_start_event_loop = false;

        let mut msg_idx = 0;
        while msg_idx < msgs.len() {
            let msg = take(&mut msgs[msg_idx]);
            msg_idx += 1;

            match msg {
                WorkerMessage::Noop => {}
                WorkerMessage::Exit => {
                    reset_event_loop(
                        &mut current_loop_id,
                        &mut event_loop_handle,
                        &mut events_from_loop,
                    )
                    .await;
                    return;
                }
                WorkerMessage::Event { event } => {
                    events.insert(event);
                }
                WorkerMessage::Events { events: new_events } => events.extend(new_events),
                WorkerMessage::EventFromLoop { loop_id, event } => {
                    if current_loop_id == loop_id {
                        events_from_loop.insert((loop_id, event));
                    }
                }
                WorkerMessage::EventsFromLoop {
                    loop_id,
                    events: mut new_events,
                } => {
                    if current_loop_id == loop_id {
                        for event in new_events.drain(..) {
                            events_from_loop.insert((loop_id, event));
                        }
                    }
                }
                WorkerMessage::Update { graph, func_lib } => {
                    should_start_event_loop |= event_loop_handle.is_some();
                    reset_event_loop(
                        &mut current_loop_id,
                        &mut event_loop_handle,
                        &mut events_from_loop,
                    )
                    .await;
                    update_graph = Some((graph, func_lib));
                }
                WorkerMessage::Clear => {
                    should_start_event_loop |= event_loop_handle.is_some();
                    reset_event_loop(
                        &mut current_loop_id,
                        &mut event_loop_handle,
                        &mut events_from_loop,
                    )
                    .await;
                    execution_graph.clear();
                    execution_graph_clear = true;
                }
                WorkerMessage::ExecuteTerminals => execute_terminals = true,
                WorkerMessage::StartEventLoop => {
                    reset_event_loop(
                        &mut current_loop_id,
                        &mut event_loop_handle,
                        &mut events_from_loop,
                    )
                    .await;
                    should_start_event_loop = true;
                }
                WorkerMessage::StopEventLoop => {
                    reset_event_loop(
                        &mut current_loop_id,
                        &mut event_loop_handle,
                        &mut events_from_loop,
                    )
                    .await;
                }
                WorkerMessage::Multi { msgs: new_msgs } => msgs.extend(new_msgs),
                WorkerMessage::ProcessingCallback { callback } => {
                    processing_callback = Some(callback)
                }
                WorkerMessage::RequestArgumentValues { node_id, callback } => {
                    let values = execution_graph.get_argument_values(&node_id);
                    callback.call(values);
                }
            }
        }

        if let Some((graph, func_lib)) = update_graph.take() {
            tracing::info!("Graph updated");
            execution_graph.update(&graph, &func_lib);
            execution_graph_clear = false;
        }

        events.extend(
            events_from_loop
                .drain()
                .filter_map(|(loop_id, event)| (loop_id == current_loop_id).then_some(event)),
        );

        if execute_terminals || !events.is_empty() {
            if execution_graph_clear {
                tracing::error!("Execution graph is clear, cannot execute graph");
            } else {
                let result = execution_graph
                    .execute(
                        execute_terminals,
                        event_loop_handle.is_some(),
                        events.drain(),
                    )
                    .await;
                (execution_callback)(result);
            }
        }

        if should_start_event_loop {
            assert!(event_loop_handle.is_none());

            if execution_graph_clear {
                tracing::error!("Execution graph is clear, cannot start event loop");
            } else {
                let result = execution_graph.execute(false, true, []).await;
                let ok = result.is_ok();
                (execution_callback)(result);

                if ok {
                    let events_triggers = collect_active_event_triggers(&mut execution_graph);

                    if !events_triggers.is_empty() {
                        event_loop_handle = Some(
                            start_event_loop(
                                worker_message_tx.clone(),
                                events_triggers,
                                event_loop_pause_gate.clone(),
                                current_loop_id,
                            )
                            .await,
                        );

                        tracing::info!("Event loop started");
                    }
                }
            }
        }

        if let Some(callback) = processing_callback.take() {
            callback.call();
        }

        drop(event_loop_pause_guard);
        tokio::task::yield_now().await;
    }
}

type EventTrigger = (EventRef, EventLambda, EventState);

async fn start_event_loop(
    worker_message_tx: UnboundedSender<WorkerMessage>,
    events: Vec<EventTrigger>,
    pause_gate: PauseGate,
    loop_id: u64,
) -> EventLoopHandle {
    assert!(!events.is_empty());

    let mut join_handles: Vec<JoinHandle<()>> = Vec::default();

    // Bounded channel provides backpressure for event lambdas.
    // When the event loop is stopped, this channel is dropped along with event_rx,
    // causing all event lambda tasks to exit. A new event loop gets a fresh channel,
    // so old events don't leak into the new one.
    let (event_tx, mut event_rx) = channel::<EventRef>(MAX_EVENTS_PER_LOOP);

    // Forwarder task: batches events from the bounded channel and forwards them
    // to the main worker queue. This decouples the bounded backpressure from the
    // unbounded worker queue while allowing efficient batching.
    let join_handle = tokio::spawn({
        let worker_message_tx = worker_message_tx.clone();
        async move {
            let mut events = Vec::default();
            let mut seen: HashSet<EventRef> = HashSet::default();
            loop {
                events.clear();
                if event_rx.recv_many(&mut events, MAX_EVENTS_PER_LOOP).await == 0 {
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

    let ready = ReadyState::new(events.len());

    // Spawn one task per event lambda. Each task repeatedly invokes its lambda
    // and sends the event to the bounded channel.
    for (event_ref, event_lambda, event_state) in events {
        let join_handle = tokio::spawn({
            let event_tx = event_tx.clone();
            let ready = ready.clone();
            let pause_gate = pause_gate.clone();

            async move {
                ready.signal();

                loop {
                    event_lambda.invoke(event_state.clone()).await;
                    let result = event_tx.send(event_ref).await;
                    if result.is_err() {
                        return;
                    }
                    // Pause gate blocks new iterations while the main worker is processing.
                    // This prevents event buildup during graph execution.
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

async fn reset_event_loop(
    current_loop_id: &mut u64,
    event_loop_handle: &mut Option<EventLoopHandle>,
    events_from_loop: &mut HashSet<(u64, EventRef)>,
) {
    if let Some(mut event_loop_handle) = event_loop_handle.take() {
        event_loop_handle.stop().await;
        tracing::info!("Event loop stopped");
    }

    *current_loop_id += 1;
    events_from_loop.clear();
}

fn collect_active_event_triggers(execution_graph: &mut ExecutionGraph) -> Vec<EventTrigger> {
    let result = execution_graph.collect_nodes_ready_for_execution();

    result
        .flat_map(|e_node| {
            e_node
                .events
                .iter()
                .enumerate()
                .filter_map(|(event_idx, event)| {
                    (!event.subscribers.is_empty() && !event.lambda.is_none()).then_some((
                        EventRef {
                            node_id: e_node.id,
                            event_idx,
                        },
                        event.lambda.clone(),
                        event.state.clone(),
                    ))
                })
        })
        .collect()
}

impl ProcessingCallback {
    pub fn new(callback: impl FnOnce() + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(callback),
        }
    }

    fn call(self) {
        (self.inner)();
    }
}

impl ArgumentValuesCallback {
    pub fn new(callback: impl FnOnce(Option<ArgumentValues>) + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(callback),
        }
    }

    fn call(self, values: Option<ArgumentValues>) {
        (self.inner)(values);
    }
}

impl EventLoopHandle {
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

impl std::fmt::Debug for ProcessingCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessingCallback").finish()
    }
}

impl std::fmt::Debug for ArgumentValuesCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArgumentValuesCallback").finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use common::output_stream::OutputStream;
    use common::pause_gate::PauseGate;
    use tokio::sync::Notify;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::{Duration, timeout};

    use crate::elements::basic_funclib::BasicFuncLib;
    use crate::elements::timers_funclib::TimersFuncLib;
    use crate::event::EventLambda;
    use crate::event_state::EventState;
    use crate::function::FuncLib;
    use crate::graph::{Graph, Node, NodeId};

    use crate::worker::{EventRef, ProcessingCallback, Worker, WorkerMessage};

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

        let timers_invoker = TimersFuncLib::default();
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
                func_lib: func_lib.clone(),
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
        let event_state = EventState::default();

        let (tx, mut rx) = unbounded_channel();
        let mut handle = super::start_event_loop(
            tx,
            vec![(
                EventRef {
                    node_id,
                    event_idx: 0,
                },
                event_lambda,
                event_state,
            )],
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
        let event_state = EventState::default();

        let notify_for_callback = Arc::clone(&notify);
        let callback = ProcessingCallback::new(move || {
            notify_for_callback.notify_waiters();
        });

        let (tx, mut rx) = unbounded_channel();
        let mut handle = super::start_event_loop(
            tx,
            vec![(
                EventRef {
                    node_id,
                    event_idx: 0,
                },
                event_lambda,
                event_state,
            )],
            PauseGate::default(),
            0,
        )
        .await;

        callback.call();

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
        let event_state = EventState::default();

        let pause_gate = PauseGate::default();
        let (tx, mut rx) = unbounded_channel();

        let mut handle = super::start_event_loop(
            tx,
            vec![(
                EventRef {
                    node_id,
                    event_idx: 0,
                },
                event_lambda,
                event_state,
            )],
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

        let timers_invoker = TimersFuncLib::default();
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
                func_lib: func_lib.clone(),
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

        let timers_invoker = TimersFuncLib::default();
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
                    func_lib: func_lib.clone(),
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

        let timers_invoker = TimersFuncLib::default();
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
            func_lib: func_lib.clone(),
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
                func_lib: func_lib.clone(),
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

        let timers_invoker = TimersFuncLib::default();
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
                func_lib: func_lib.clone(),
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
        use std::sync::atomic::{AtomicBool, Ordering};

        let timers_invoker = TimersFuncLib::default();
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
                func_lib: func_lib.clone(),
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

        let callback_invoked = Arc::new(AtomicBool::new(false));
        let callback_invoked_clone = Arc::clone(&callback_invoked);

        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        worker.send(WorkerMessage::RequestArgumentValues {
            node_id: frame_event_node_id,
            callback: super::ArgumentValuesCallback::new(move |values| {
                callback_invoked_clone.store(true, Ordering::SeqCst);
                tx.try_send(values).ok();
            }),
        });

        let values = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("Callback timeout")
            .expect("Channel closed");

        assert!(callback_invoked.load(Ordering::SeqCst));
        assert!(values.is_some());

        worker.exit();
    }

    #[tokio::test]
    async fn processing_callback_is_invoked_after_execution() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let timers_invoker = TimersFuncLib::default();
        let basic_invoker = BasicFuncLib::default();

        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph(&func_lib);
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx.try_send(result).ok();
        });

        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = Arc::clone(&callback_count);

        worker.send_many([
            WorkerMessage::Update {
                graph: graph.clone(),
                func_lib: func_lib.clone(),
            },
            WorkerMessage::Event {
                event: EventRef {
                    node_id: frame_event_node_id,
                    event_idx: 0,
                },
            },
            WorkerMessage::ProcessingCallback {
                callback: ProcessingCallback::new(move || {
                    callback_count_clone.fetch_add(1, Ordering::SeqCst);
                }),
            },
        ]);

        // Wait for execution
        let _ = compute_finish_rx.recv().await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(callback_count.load(Ordering::SeqCst), 1);

        worker.exit();
    }

    #[tokio::test]
    async fn update_restarts_event_loop_if_running() {
        let output_stream = OutputStream::new();

        let timers_invoker = TimersFuncLib::default();
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
                func_lib: func_lib.clone(),
            },
            WorkerMessage::StartEventLoop,
        ]);

        let _ = compute_finish_rx.recv().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(worker.is_event_loop_started());

        // Update graph - should restart event loop
        worker.send(WorkerMessage::Update {
            graph: graph.clone(),
            func_lib: func_lib.clone(),
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
        let event_state = EventState::default();

        let pause_gate = PauseGate::default();
        let (tx, mut rx) = unbounded_channel();

        // Start first event loop with loop_id = 0
        let mut handle = super::start_event_loop(
            tx.clone(),
            vec![(
                EventRef {
                    node_id,
                    event_idx: 0,
                },
                event_lambda.clone(),
                event_state.clone(),
            )],
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
        let new_event_state = EventState::default();

        let mut new_handle = super::start_event_loop(
            tx,
            vec![(
                EventRef {
                    node_id,
                    event_idx: 0,
                },
                new_event_lambda,
                new_event_state,
            )],
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
}
