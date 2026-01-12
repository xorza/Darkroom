use std::collections::VecDeque;

use crate::event::EventLambda;
use crate::execution_graph::{ExecutionGraph, ExecutionStats, Result};
use crate::function::FuncLib;
use crate::graph::{Graph, NodeId};
use common::Shared;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::{JoinHandle, JoinSet};
use tracing::error;

#[derive(Debug)]
pub enum WorkerMessage {
    Exit,
    Event { event_id: EventId },
    Events { event_ids: Vec<EventId> },
    Update { graph: Graph, func_lib: FuncLib },
    Clear,
    ExecuteTerminals,
    StartEventLoop,
    StopEventLoop,
    Multi { msgs: Vec<WorkerMessage> },
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
    event_task_handle: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone)]
pub struct EventLoopHandle {
    inner: Shared<EventLoopInner>,
}

impl Worker {
    pub fn new<Callback>(callback: Callback) -> Self
    where
        Callback: Fn(Result<ExecutionStats>) + Send + 'static,
    {
        let callback: Shared<Callback> = Shared::new(callback);
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
    pub fn execute_terminals(&self) {
        self.send(WorkerMessage::ExecuteTerminals);
    }

    pub fn update(&self, graph: Graph, func_lib: FuncLib) {
        self.send(WorkerMessage::Update { graph, func_lib });
    }

    pub fn exit(&mut self) {
        self.send(WorkerMessage::Exit);

        if let Some(_thread_handle) = self.thread_handle.take() {
            // thread_handle.await.expect("Worker thread failed to join");
        }
    }

    pub fn event(&self, event_id: EventId) {
        self.send(WorkerMessage::Event { event_id });
    }

    pub fn execute_events<T: IntoIterator<Item = EventId>>(&self, event_ids: T) {
        let event_ids: Vec<EventId> = event_ids.into_iter().collect();
        self.send(WorkerMessage::Events { event_ids });
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.thread_handle.is_some() {
            error!("Worker dropped while the thread is still running; call Worker::exit() first");
        }
    }
}

impl EventLoopHandle {
    pub async fn stop(&self) {
        let mut inner = self.inner.lock().await;
        if let Some(event_task_handle) = inner.event_task_handle.take() {
            event_task_handle.abort();
        }
    }
}

enum EventLoopCommand {
    None,
    Start,
    Stop,
}

async fn worker_loop<Callback>(
    mut rx: UnboundedReceiver<WorkerMessage>,
    tx: UnboundedSender<WorkerMessage>,
    callback: Shared<Callback>,
) where
    Callback: Fn(Result<ExecutionStats>) + Send + 'static,
{
    let mut execution_graph = ExecutionGraph::default();
    let mut msgs: VecDeque<WorkerMessage> = VecDeque::default();

    let mut events: Vec<EventId> = Vec::default();
    let mut event_loop_handle: Option<EventLoopHandle> = None;

    'worker: loop {
        events.clear();

        let msg = rx.recv().await;
        let Some(msg) = msg else { break };
        msgs.push_back(msg);

        loop {
            match rx.try_recv() {
                Ok(msg) => msgs.push_back(msg),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'worker,
            }
        }

        let mut execute_terminals: bool = false;
        let mut event_loop_cmd = EventLoopCommand::None;
        let mut update_graph: Option<(Graph, FuncLib)> = None;

        while let Some(msg) = msgs.pop_front() {
            match msg {
                WorkerMessage::Exit => break 'worker,
                WorkerMessage::Event { event_id } => events.push(event_id),
                WorkerMessage::Events { event_ids } => events.extend(event_ids),
                WorkerMessage::Update { graph, func_lib } => {
                    events.clear();
                    update_graph = Some((graph, func_lib));
                }
                WorkerMessage::Clear => {
                    events.clear();
                    execution_graph.clear();
                }
                WorkerMessage::ExecuteTerminals => execute_terminals = true,
                WorkerMessage::StartEventLoop => event_loop_cmd = EventLoopCommand::Start,
                WorkerMessage::StopEventLoop => event_loop_cmd = EventLoopCommand::Stop,
                WorkerMessage::Multi { msgs: new_msgs } => msgs.extend(new_msgs),
            }
        }

        if let Some((graph, func_lib)) = update_graph.take() {
            execution_graph.update(&graph, &func_lib);
        }

        if execute_terminals || !events.is_empty() {
            let result = execution_graph
                .execute(execute_terminals, events.drain(..))
                .await;
            (callback.lock().await)(result);
        }

        match event_loop_cmd {
            EventLoopCommand::None => {}
            EventLoopCommand::Start => {
                stop_event_loop(&mut event_loop_handle).await;
                let mut events: Vec<(NodeId, EventLambda)> = Vec::default();
                for e_node in execution_graph.e_nodes.iter() {
                    if !e_node.event_lambda.is_none() {
                        events.push((e_node.id, e_node.event_lambda.clone()));
                    }
                }
                if !events.is_empty() {
                    event_loop_handle = Some(start_event_loop(tx.clone(), events));
                }
            }
            EventLoopCommand::Stop => stop_event_loop(&mut event_loop_handle).await,
        }
    }
}

fn start_event_loop(
    tx: UnboundedSender<WorkerMessage>,
    events: Vec<(NodeId, EventLambda)>,
) -> EventLoopHandle {
    assert!(!events.is_empty());

    let event_task_handle = tokio::spawn({
        async move {
            let mut pending = JoinSet::new();
            for (node_id, event_lambda) in events {
                spawn_event_task(&mut pending, node_id, event_lambda);
            }

            let mut event_ids: Vec<EventId> = Vec::default();
            let mut events: Vec<(NodeId, EventLambda)> = Vec::default();

            while let Some(result) = pending.join_next().await {
                let (event_lambda, node_id, event_idx) =
                    result.expect("Event loop task should complete");
                event_ids.push(EventId { node_id, event_idx });
                events.push((node_id, event_lambda));

                while let Some(result) = pending.try_join_next() {
                    let (event_lambda, node_id, event_idx) =
                        result.expect("Event loop task should complete");
                    event_ids.push(EventId { node_id, event_idx });
                    events.push((node_id, event_lambda));
                }

                let result = tx.send(WorkerMessage::Events {
                    event_ids: std::mem::take(&mut event_ids),
                });
                if result.is_err() {
                    return;
                }

                for (node_id, event_lambda) in events.drain(..) {
                    spawn_event_task(&mut pending, node_id, event_lambda);
                }
            }
        }
    });

    EventLoopHandle {
        inner: Shared::new(EventLoopInner {
            event_task_handle: Some(event_task_handle),
        }),
    }
}

fn spawn_event_task(
    pending: &mut JoinSet<(EventLambda, NodeId, usize)>,
    node_id: NodeId,
    event_lambda: EventLambda,
) {
    pending.spawn(async move {
        let event_idx = event_lambda.invoke().await;
        (event_lambda, node_id, event_idx)
    });
}

async fn stop_event_loop(event_loop_handle: &mut Option<EventLoopHandle>) {
    if let Some(event_loop_handle) = event_loop_handle.take() {
        event_loop_handle.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use common::output_stream::OutputStream;
    use tokio::sync::mpsc::unbounded_channel;

    use crate::elements::basic_funclib::BasicFuncLib;
    use crate::elements::timers_funclib::{FRAME_EVENT_FUNC_ID, TimersFuncLib};
    use crate::event::EventLambda;
    use crate::function::FuncId;
    use crate::graph::{Binding, Graph, Input, Node, NodeBehavior};
    use crate::graph::{Event, NodeId};

    use crate::worker::{EventId, Worker, WorkerMessage};

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
        worker.event(EventId {
            node_id: frame_event_node_id,
            event_idx: 0,
        });

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes.len(), 3);
        assert_eq!(output_stream.take().await, ["1"]);

        worker.event(EventId {
            node_id: frame_event_node_id,
            event_idx: 0,
        });

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes.len(), 3);
        assert_eq!(output_stream.take().await, ["2"]);

        worker.event(EventId {
            node_id: frame_event_node_id,
            event_idx: 0,
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
        let handle = super::start_event_loop(tx, vec![(node_id, event_lambda)]);

        let event_id = EventId {
            node_id,
            event_idx: 1,
        };

        let msg = rx.recv().await.expect("Expected event loop message");
        let WorkerMessage::Events { event_ids } = msg else {
            panic!("Expected WorkerMessage::Events");
        };
        assert_eq!(event_ids, vec![event_id]);

        handle.stop().await;
    }
}
