use std::collections::HashSet;

use crate::event::EventLambda;
use crate::execution_graph::{ArgumentValues, ExecutionGraph, ExecutionStats, Result};
use crate::function::FuncLib;
use crate::graph::{Graph, NodeId};
use common::pause_gate::PauseGate;
use common::{ReadyState, Shared};
use serde::{Deserialize, Serialize};
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
    inner: Box<dyn Fn() + Send + Sync + 'static>,
}

pub struct ArgumentValuesCallback {
    inner: Box<dyn FnOnce(Option<ArgumentValues>) + Send + 'static>,
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
        let callback: Shared<ExecutionCallback> = Shared::new(callback);
        let (tx, rx) = unbounded_channel::<WorkerMessage>();
        let thread_handle: JoinHandle<()> = tokio::spawn({
            let tx = tx.clone();
            async move {
                worker_loop(rx, tx, callback).await;
            }
        });

        Self {
            thread_handle: Some(thread_handle),
            tx,
        }
    }

    pub fn send(&self, msg: WorkerMessage) {
        self.tx
            .send(msg)
            .expect("Failed to send message to worker, it probably exited");
    }

    pub fn send_many<T: IntoIterator<Item = WorkerMessage>>(&self, msgs: T) {
        self.tx
            .send(WorkerMessage::Multi {
                msgs: msgs.into_iter().collect(),
            })
            .expect("Failed to send message to worker, it probably exited");
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

#[derive(Debug, Clone)]
enum EventLoopCommand {
    None,
    Start,
}

async fn worker_loop<Callback>(
    mut worker_message_rx: UnboundedReceiver<WorkerMessage>,
    worker_message_tx: UnboundedSender<WorkerMessage>,
    execution_callback: Shared<Callback>,
) where
    Callback: Fn(Result<ExecutionStats>) + Send + 'static,
{
    let mut execution_graph = ExecutionGraph::default();

    let mut msgs: Vec<WorkerMessage> = Vec::with_capacity(MAX_EVENTS_PER_LOOP);

    let mut events: HashSet<EventRef> = HashSet::default();
    let mut event_loop_handle: Option<EventLoopHandle> = None;
    let mut processing_callback: Option<ProcessingCallback> = None;
    let event_loop_pause_gate = PauseGate::default();

    loop {
        assert!(events.is_empty());
        assert!(processing_callback.is_none());

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
        let mut event_loop_cmd = EventLoopCommand::None;
        let mut update_graph: Option<(Graph, FuncLib)> = None;

        let mut msg_idx = 0;
        while msg_idx < msgs.len() {
            let msg = std::mem::take(&mut msgs[msg_idx]);
            msg_idx += 1;

            match msg {
                WorkerMessage::Noop => {}
                WorkerMessage::Exit => {
                    stop_event_loop(&mut event_loop_handle).await;
                    return;
                }
                WorkerMessage::Event { event } => {
                    events.insert(event);
                }
                WorkerMessage::Events { events: new_events } => events.extend(new_events),
                WorkerMessage::Update { graph, func_lib } => {
                    stop_event_loop(&mut event_loop_handle).await;
                    events.clear();
                    update_graph = Some((graph, func_lib));
                }
                WorkerMessage::Clear => {
                    stop_event_loop(&mut event_loop_handle).await;
                    events.clear();
                    execution_graph.clear();
                }
                WorkerMessage::ExecuteTerminals => execute_terminals = true,
                WorkerMessage::StartEventLoop => event_loop_cmd = EventLoopCommand::Start,
                WorkerMessage::StopEventLoop => {
                    stop_event_loop(&mut event_loop_handle).await;
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
            if event_loop_handle.is_some() && matches!(event_loop_cmd, EventLoopCommand::None) {
                event_loop_cmd = EventLoopCommand::Start;
            }

            tracing::info!("Graph updated");
            execution_graph.update(&graph, &func_lib);
        }

        if execute_terminals || !events.is_empty() {
            let result = execution_graph
                .execute(
                    execute_terminals,
                    event_loop_handle.is_some(),
                    events.drain(),
                )
                .await;
            (execution_callback.lock().await)(result);
        }

        match event_loop_cmd {
            EventLoopCommand::None => {}
            EventLoopCommand::Start => {
                stop_event_loop(&mut event_loop_handle).await;

                let result = execution_graph.execute(false, true, []).await;
                let ok = result.is_ok();
                (execution_callback.lock().await)(result);

                if ok {
                    let events_triggers = collect_active_event_triggers(&mut execution_graph);

                    if !events_triggers.is_empty() {
                        event_loop_handle = Some(
                            start_event_loop(
                                worker_message_tx.clone(),
                                events_triggers,
                                event_loop_pause_gate.clone(),
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

async fn start_event_loop(
    worker_message_tx: UnboundedSender<WorkerMessage>,
    events: Vec<(EventRef, EventLambda)>,
    pause_gate: PauseGate,
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
                let result = if events.len() == 1 {
                    worker_message_tx.send(WorkerMessage::Event { event: events[0] })
                } else {
                    seen.clear();
                    events.retain(|event| seen.insert(*event));

                    worker_message_tx.send(WorkerMessage::Events {
                        events: std::mem::take(&mut events),
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
    for (event_ref, event_lambda) in events {
        let join_handle = tokio::spawn({
            let event_tx = event_tx.clone();
            let ready = ready.clone();
            let pause_gate = pause_gate.clone();

            async move {
                ready.signal();

                loop {
                    event_lambda.invoke().await;

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

async fn stop_event_loop(event_loop_handle: &mut Option<EventLoopHandle>) {
    if let Some(mut event_loop_handle) = event_loop_handle.take() {
        event_loop_handle.stop().await;
        tracing::info!("Event loop stopped");
    }
}

fn collect_active_event_triggers(
    execution_graph: &mut ExecutionGraph,
) -> Vec<(EventRef, EventLambda)> {
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
                    ))
                })
        })
        .collect()
}

impl ProcessingCallback {
    pub fn new(callback: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            inner: Box::new(callback),
        }
    }

    fn call(&self) {
        (self.inner)();
    }
}

impl ArgumentValuesCallback {
    pub fn new(callback: impl FnOnce(Option<ArgumentValues>) + Send + 'static) -> Self {
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
        let event_lambda = EventLambda::new(|| Box::pin(async move {}));

        let (tx, mut rx) = unbounded_channel();
        let mut handle = super::start_event_loop(
            tx,
            vec![(
                EventRef {
                    node_id,
                    event_idx: 0,
                },
                event_lambda,
            )],
            PauseGate::default(),
        )
        .await;

        let msg = rx.recv().await.expect("Expected event loop message");
        let WorkerMessage::Event { event } = msg else {
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
        let event_lambda = EventLambda::new(move || {
            let notify = Arc::clone(&notify_for_event);
            Box::pin(async move {
                notify.notified().await;
            })
        });

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
            )],
            PauseGate::default(),
        )
        .await;

        callback.call();

        let msg = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("Expected event message")
            .expect("Event channel closed");
        let WorkerMessage::Event { event } = msg else {
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

        let event_lambda = EventLambda::new(move || {
            let invoke_count = Arc::clone(&invoke_count_clone);
            Box::pin(async move {
                invoke_count.fetch_add(1, Ordering::SeqCst);
            })
        });

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
            )],
            pause_gate.clone(),
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
}
