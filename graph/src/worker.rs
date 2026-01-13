use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use crate::event::EventLambda;
use crate::execution_graph::{ExecutionGraph, ExecutionStats, Result};
use crate::function::FuncLib;
use crate::graph::{Graph, NodeId};
use common::{ReadyState, Shared};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, channel, unbounded_channel};
use tokio::task::{JoinHandle, JoinSet};
use tracing::error;

const MAX_EVENTS_PER_LOOP: usize = 10;

#[derive(Debug)]
pub enum WorkerMessage {
    Exit,
    Event { event_id: EventId },
    Events { event_ids: Vec<EventId> },
    Update { graph: Graph, func_lib: FuncLib },
    Clear,
    ExecuteTerminals,
    StartEventLoop { callback: EventLoopCallback },
    StopEventLoop,
    Multi { msgs: Vec<WorkerMessage> },
}

#[derive(Clone)]
pub struct EventLoopCallback {
    inner: Option<Arc<dyn Fn() + Send + Sync + 'static>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId {
    pub node_id: NodeId,
    pub event_idx: usize,
}

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<WorkerMessage>,
}

#[derive(Debug)]
struct EventLoopInner {
    event_task_handle: JoinHandle<()>,
    event_task_set: JoinSet<()>,
}

#[derive(Debug, Clone)]
struct EventLoopHandle {
    inner: Shared<Option<EventLoopInner>>,
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
        self.tx.send(msg).unwrap();
    }

    pub fn update(&self, graph: Graph, func_lib: FuncLib) {
        self.send(WorkerMessage::Update { graph, func_lib });
    }

    pub fn exit(&mut self) {
        self.send(WorkerMessage::Exit);

        if let Some(thread_handle) = self.thread_handle.take() {
            thread_handle.abort();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(thread_handle) = self.thread_handle.take() {
            thread_handle.abort();
            error!("Worker dropped while the thread is still running; call Worker::exit() first");
        }
    }
}

#[derive(Debug, Clone)]
enum EventLoopCommand {
    None,
    Start { callback: EventLoopCallback },
    Stop,
}

async fn worker_loop<Callback>(
    mut worker_message_rx: UnboundedReceiver<WorkerMessage>,
    worker_message_tx: UnboundedSender<WorkerMessage>,
    callback: Shared<Callback>,
) where
    Callback: Fn(Result<ExecutionStats>) + Send + 'static,
{
    let mut execution_graph = ExecutionGraph::default();

    let mut msgs: VecDeque<WorkerMessage> = VecDeque::with_capacity(MAX_EVENTS_PER_LOOP);
    let mut msgs_vec: Vec<WorkerMessage> = Vec::with_capacity(MAX_EVENTS_PER_LOOP);

    let mut event_ids: HashSet<EventId> = HashSet::default();
    let mut event_loop_handle: Option<EventLoopHandle> = None;

    loop {
        assert!(msgs.is_empty());
        assert!(msgs_vec.is_empty());
        assert!(event_ids.is_empty());

        if worker_message_rx
            .recv_many(&mut msgs_vec, MAX_EVENTS_PER_LOOP)
            .await
            == 0
        {
            return;
        }
        msgs.extend(msgs_vec.drain(..));

        let mut execute_terminals: bool = false;
        let mut event_loop_cmd = EventLoopCommand::None;
        let mut update_graph: Option<(Graph, FuncLib)> = None;

        while let Some(msg) = msgs.pop_front() {
            match msg {
                WorkerMessage::Exit => return,
                WorkerMessage::Event { event_id } => {
                    event_ids.insert(event_id);
                }
                WorkerMessage::Events {
                    event_ids: new_event_ids,
                } => event_ids.extend(new_event_ids),
                WorkerMessage::Update { graph, func_lib } => {
                    event_ids.clear();
                    update_graph = Some((graph, func_lib));
                }
                WorkerMessage::Clear => {
                    event_ids.clear();
                    execution_graph.clear();
                }
                WorkerMessage::ExecuteTerminals => execute_terminals = true,
                WorkerMessage::StartEventLoop { callback } => {
                    event_loop_cmd = EventLoopCommand::Start { callback };
                }
                WorkerMessage::StopEventLoop => event_loop_cmd = EventLoopCommand::Stop,
                WorkerMessage::Multi { msgs: new_msgs } => msgs.extend(new_msgs),
            }
        }

        if let Some((graph, func_lib)) = update_graph.take() {
            if event_loop_handle.is_some() && matches!(event_loop_cmd, EventLoopCommand::None) {
                event_loop_cmd = EventLoopCommand::Start {
                    callback: EventLoopCallback::none(),
                };
            }
            stop_event_loop(&mut event_loop_handle).await;
            execution_graph.update(&graph, &func_lib);
        }

        if execute_terminals || !event_ids.is_empty() {
            let result = execution_graph
                .execute(execute_terminals, event_ids.drain())
                .await;
            (callback.lock().await)(result);
        }

        match event_loop_cmd {
            EventLoopCommand::None => {}
            EventLoopCommand::Start { callback } => {
                stop_event_loop(&mut event_loop_handle).await;
                let events: Vec<(NodeId, EventLambda)> = execution_graph
                    .e_nodes
                    .iter()
                    .filter(|e_node| !e_node.event_lambda.is_none())
                    .map(|e_node| (e_node.id, e_node.event_lambda.clone()))
                    .collect();
                if !events.is_empty() {
                    event_loop_handle =
                        Some(start_event_loop(worker_message_tx.clone(), events, callback).await);
                }
            }
            EventLoopCommand::Stop => stop_event_loop(&mut event_loop_handle).await,
        }
    }
}

async fn start_event_loop(
    worker_message_tx: UnboundedSender<WorkerMessage>,
    events: Vec<(NodeId, EventLambda)>,
    callback: EventLoopCallback,
) -> EventLoopHandle {
    assert!(!events.is_empty());

    let (event_tx, mut event_rx) = channel::<EventId>(MAX_EVENTS_PER_LOOP);
    let event_task_handle = tokio::spawn({
        let tx = worker_message_tx.clone();
        async move {
            let mut event_ids = Vec::default();
            loop {
                event_ids.clear();
                if event_rx
                    .recv_many(&mut event_ids, MAX_EVENTS_PER_LOOP)
                    .await
                    == 0
                {
                    return;
                }
                let result = if event_ids.len() == 1 {
                    tx.send(WorkerMessage::Event {
                        event_id: event_ids[0].clone(),
                    })
                } else {
                    tx.send(WorkerMessage::Events {
                        event_ids: std::mem::take(&mut event_ids),
                    })
                };

                if result.is_err() {
                    return;
                }
            }
        }
    });

    let ready = ReadyState::new(events.len());

    let mut event_task_set = JoinSet::new();
    for (node_id, event_lambda) in events {
        event_task_set.spawn({
            let event_tx = event_tx.clone();
            let ready = ready.clone();

            async move {
                ready.signal();

                loop {
                    let event_idx = event_lambda.invoke().await;
                    let result = event_tx.send(EventId { node_id, event_idx }).await;
                    if result.is_err() {
                        return;
                    }

                    tokio::task::yield_now().await;
                }
            }
        });
    }

    ready.wait().await;
    tokio::task::yield_now().await;
    callback.call();

    EventLoopHandle {
        inner: Shared::new(Some(EventLoopInner {
            event_task_handle,
            event_task_set,
        })),
    }
}

async fn stop_event_loop(event_loop_handle: &mut Option<EventLoopHandle>) {
    if let Some(event_loop_handle) = event_loop_handle.take() {
        event_loop_handle.stop().await;
    }
}

impl EventLoopCallback {
    pub fn new(callback: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            inner: Some(Arc::new(callback)),
        }
    }

    pub fn none() -> Self {
        Self { inner: None }
    }

    pub fn call(&self) {
        if let Some(inner) = &self.inner {
            (inner)();
        }
    }
}

impl EventLoopHandle {
    async fn stop(&self) {
        let Some(mut inner) = self.inner.lock().await.take() else {
            return;
        };

        inner.event_task_handle.abort();
        inner.event_task_set.abort_all();
        while let Some(result) = inner.event_task_set.join_next().await {
            if let Err(err) = result {
                if err.is_panic() {
                    std::panic::resume_unwind(err.into_panic());
                }
                assert!(err.is_cancelled(), "event task join error: {err}");
            }
        }
    }
}

impl std::fmt::Debug for EventLoopCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventLoopCallback").finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use common::output_stream::OutputStream;
    use tokio::sync::Notify;
    use tokio::sync::mpsc::unbounded_channel;
    use tokio::time::{Duration, timeout};

    use crate::elements::basic_funclib::BasicFuncLib;
    use crate::elements::timers_funclib::{FRAME_EVENT_FUNC_ID, TimersFuncLib};
    use crate::event::EventLambda;
    use crate::function::FuncId;
    use crate::graph::{Binding, Graph, Input, Node, NodeBehavior};
    use crate::graph::{Event, NodeId};

    use crate::worker::{EventId, EventLoopCallback, Worker, WorkerMessage};

    fn log_frame_no_graph() -> Graph {
        let mut graph = Graph::default();

        let frame_event_node_id: NodeId = "e69c3f32-ac66-4447-a3f6-9e8528c5d830".into();
        let frame_event_func_id: FuncId = FRAME_EVENT_FUNC_ID;

        let float_to_string_node_id: NodeId = "eb6590aa-229d-4874-abba-37c56f5b97fa".into();
        let float_to_string_func_id: FuncId = "01896a88-bf15-dead-4a15-5969da5a9e65".into();

        let print_node_id: NodeId = "8be72298-dece-4a5f-8a1d-d2dee1e791d3".into();
        let print_func_id: FuncId = "01896910-0790-ad1b-aa12-3f1437196789".into();

        graph.add(Node {
            id: frame_event_node_id,
            func_id: frame_event_func_id,
            name: "frame event".to_string(),
            behavior: NodeBehavior::AsFunction,
            inputs: vec![Input {
                binding: Binding::None,
            }],
            events: vec![Event {
                subscribers: vec![print_node_id],
            }],
        });

        graph.add(Node {
            id: float_to_string_node_id,
            func_id: float_to_string_func_id,
            name: "float to string".to_string(),
            behavior: NodeBehavior::AsFunction,
            inputs: vec![Input {
                binding: (frame_event_node_id, 1).into(),
            }],
            events: vec![],
        });

        graph.add(Node {
            id: print_node_id,
            func_id: print_func_id,
            name: "print".to_string(),
            behavior: NodeBehavior::AsFunction,
            inputs: vec![Input {
                binding: (float_to_string_node_id, 0).into(),
            }],
            events: vec![],
        });

        graph
    }

    #[tokio::test]
    async fn test_worker() -> anyhow::Result<()> {
        let output_stream = OutputStream::new();

        let timers_invoker = TimersFuncLib::default();
        let basic_invoker = BasicFuncLib::with_output_stream(&output_stream).await;

        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph();
        let frame_event_node_id = graph.by_name("frame event").unwrap().id;

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx
                .try_send(result)
                .expect("Failed to send a compute callback event");
        });

        worker.update(graph.clone(), func_lib.clone());
        worker.send(WorkerMessage::Event {
            event_id: EventId {
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
        assert_eq!(output_stream.take().await, ["1"]);

        worker.send(WorkerMessage::Event {
            event_id: EventId {
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
            event_id: EventId {
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
        let event_lambda = EventLambda::new(|| Box::pin(async move { 1 }));

        let (tx, mut rx) = unbounded_channel();
        let handle = super::start_event_loop(
            tx,
            vec![(node_id, event_lambda)],
            EventLoopCallback::new(|| {}),
        )
        .await;

        let msg = rx.recv().await.expect("Expected event loop message");
        let WorkerMessage::Event { event_id } = msg else {
            panic!("Expected WorkerMessage::Event");
        };
        assert_eq!(
            event_id,
            EventId {
                node_id,
                event_idx: 1
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
                0
            })
        });

        let notify_for_callback = Arc::clone(&notify);
        let callback = EventLoopCallback::new(move || {
            notify_for_callback.notify_waiters();
        });

        let (tx, mut rx) = unbounded_channel();
        let handle = super::start_event_loop(tx, vec![(node_id, event_lambda)], callback).await;

        let msg = timeout(Duration::from_millis(200), rx.recv())
            .await
            .expect("Expected event message")
            .expect("Event channel closed");
        let WorkerMessage::Event { event_id } = msg else {
            panic!("Expected WorkerMessage::Event");
        };
        assert_eq!(
            event_id,
            EventId {
                node_id,
                event_idx: 0
            }
        );

        handle.stop().await;
    }
}
