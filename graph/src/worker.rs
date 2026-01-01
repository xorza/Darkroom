use crate::event::EventId;
use crate::execution_graph::{ExecutionGraph, ExecutionResult, ExecutionStats};
use crate::function::FuncLib;
use crate::graph::{Graph, NodeId};
use common::Shared;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::error;

#[derive(Debug)]
enum WorkerMessage {
    Exit,
    Stop,
    Event,
    RunOnce { graph: Graph, func_lib: FuncLib },
    RunLoop { graph: Graph, func_lib: FuncLib },
    InvalidateCaches(Vec<NodeId>),
}

type EventQueue = Shared<Vec<EventId>>;

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: Sender<WorkerMessage>,
}

impl Worker {
    pub fn new<Callback>(compute_callback: Callback) -> Self
    where
        Callback: Fn(ExecutionResult<ExecutionStats>) + Send + 'static,
    {
        let compute_callback: Shared<Callback> = Shared::new(compute_callback);

        let (tx, rx) = channel::<WorkerMessage>(10);
        let thread_handle: JoinHandle<()> = tokio::spawn(async move {
            Self::worker_loop(rx, compute_callback.clone()).await;
        });

        Self {
            thread_handle: Some(thread_handle),
            tx,
        }
    }

    async fn worker_loop<Callback>(
        mut rx: Receiver<WorkerMessage>,
        compute_callback: Shared<Callback>,
    ) where
        Callback: Fn(ExecutionResult<ExecutionStats>) + Send + 'static,
    {
        let mut execution_graph = ExecutionGraph::default();
        let mut message: Option<WorkerMessage> = None;

        loop {
            if message.is_none() {
                message = rx.recv().await;
            }
            if message.is_none() {
                break;
            }

            match message.take().unwrap() {
                WorkerMessage::Stop | WorkerMessage::Event => panic!("Unexpected event message"),

                WorkerMessage::Exit => break,

                WorkerMessage::InvalidateCaches(node_ids) => {
                    execution_graph.invalidate_recurisevly(node_ids);
                }

                WorkerMessage::RunOnce { graph, func_lib } => {
                    if let Err(err) = execution_graph.update(&graph, &func_lib) {
                        (compute_callback.lock().await)(Err(err));
                        continue;
                    }

                    let result = execution_graph.execute().await;
                    (compute_callback.lock().await)(result);
                }

                WorkerMessage::RunLoop { graph, func_lib } => {
                    if let Err(err) = execution_graph.update(&graph, &func_lib) {
                        (compute_callback.lock().await)(Err(err));
                        continue;
                    }

                    message = Self::event_subloop(
                        &mut execution_graph,
                        &mut rx,
                        compute_callback.clone(),
                    )
                    .await;
                }
            }
        }
    }

    async fn event_subloop<Callback>(
        execution_graph: &mut ExecutionGraph,
        worker_rx: &mut Receiver<WorkerMessage>,
        compute_callback: Shared<Callback>,
    ) -> Option<WorkerMessage>
    where
        Callback: Fn(ExecutionResult<ExecutionStats>) + Send + 'static,
    {
        loop {
            // receive all messages and pick message with the highest priority
            // todo build stack of messages and remove unneded
            let mut msg = worker_rx.recv().await?;
            loop {
                match worker_rx.try_recv() {
                    Ok(another_msg) => {
                        // latest message has higher priority
                        if another_msg.priority() >= msg.priority() {
                            msg = another_msg;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return None,
                }
            }

            match msg {
                WorkerMessage::Stop => break None,
                WorkerMessage::Exit
                | WorkerMessage::RunOnce { .. }
                | WorkerMessage::RunLoop { .. } => {
                    break Some(msg);
                }
                WorkerMessage::InvalidateCaches(node_ids) => {
                    execution_graph.invalidate_recurisevly(node_ids);
                }
                WorkerMessage::Event => {
                    let result = execution_graph.execute().await;
                    (compute_callback.lock().await)(result);
                }
            }
        }
    }

    pub async fn invalidate_caches<I>(&mut self, node_ids: I)
    where
        I: IntoIterator<Item = NodeId>,
    {
        self.tx
            .send(WorkerMessage::InvalidateCaches(
                node_ids.into_iter().collect(),
            ))
            .await
            .expect("Failed to send invalidate_caches message");
    }
    pub async fn run_once(&mut self, graph: Graph, func_lib: FuncLib) {
        self.tx
            .send(WorkerMessage::RunOnce { graph, func_lib })
            .await
            .expect("Failed to send run_once message");
    }
    pub async fn run_loop(&mut self, graph: Graph, func_lib: FuncLib) {
        self.tx
            .send(WorkerMessage::RunLoop { graph, func_lib })
            .await
            .expect("Failed to send run_loop message");
    }
    pub async fn stop(&mut self) {
        self.tx
            .send(WorkerMessage::Stop)
            .await
            .expect("Failed to send stop message");
    }
    pub async fn exit(&mut self) {
        self.tx
            .send(WorkerMessage::Exit)
            .await
            .expect("Failed to send exit message");

        if let Some(thread_handle) = self.thread_handle.take() {
            thread_handle.await.expect("Worker thread failed to join");
        }
    }
    pub async fn event(&mut self) {
        self.tx
            .send(WorkerMessage::Event)
            .await
            .expect("Failed to send event message");
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.thread_handle.is_some() {
            error!("Worker dropped while the thread is still running; call Worker::exit() first");
        }
    }
}

impl WorkerMessage {
    fn priority(&self) -> u8 {
        match self {
            WorkerMessage::Exit => 255,
            WorkerMessage::Stop => 128,
            WorkerMessage::InvalidateCaches(_) => 96,
            WorkerMessage::RunOnce { .. } | WorkerMessage::RunLoop { .. } => 64,
            WorkerMessage::Event => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use common::output_stream::OutputStream;
    use common::Shared;
    use tokio::sync::Mutex;

    use crate::data::StaticValue;
    use crate::elements::basic_invoker::BasicInvoker;
    use crate::elements::timers_invoker::TimersInvoker;
    use crate::function::FuncId;
    use crate::graph::NodeId;
    use crate::graph::{Binding, Graph, Input, Node, NodeBehavior};
    use crate::prelude::ExecutionGraph;
    use crate::worker::Worker;

    fn log_frame_no_graph() -> Graph {
        let mut graph = Graph::default();

        let frame_event_node_id: NodeId = "e69c3f32-ac66-4447-a3f6-9e8528c5d830".parse().unwrap();
        let frame_event_func_id: FuncId = "01897c92-d605-5f5a-7a21-627ed74824ff".parse().unwrap();

        let float_to_string_node_id: NodeId =
            "eb6590aa-229d-4874-abba-37c56f5b97fa".parse().unwrap();
        let float_to_string_func_id: FuncId =
            "01896a88-bf15-dead-4a15-5969da5a9e65".parse().unwrap();

        let print_node_id: NodeId = "8be72298-dece-4a5f-8a1d-d2dee1e791d3".parse().unwrap();
        let print_func_id: FuncId = "01896910-0790-ad1b-aa12-3f1437196789".parse().unwrap();

        graph.add(Node {
            id: frame_event_node_id,
            func_id: frame_event_func_id,
            name: "frame event".to_string(),
            behavior: NodeBehavior::AsFunction,
            terminal: false,
            inputs: vec![Input {
                binding: Binding::None,
                default_value: None,
            }],
            events: vec![],
        });

        graph.add(Node {
            id: float_to_string_node_id,
            func_id: float_to_string_func_id,
            name: "float to string".to_string(),
            behavior: NodeBehavior::AsFunction,
            terminal: false,
            inputs: vec![Input {
                binding: (frame_event_node_id, 1).into(),
                default_value: None,
            }],
            events: vec![],
        });

        graph.add(Node {
            id: print_node_id,
            func_id: print_func_id,
            name: "print".to_string(),
            behavior: NodeBehavior::AsFunction,
            terminal: true,
            inputs: vec![Input {
                binding: (float_to_string_node_id, 0).into(),
                default_value: None,
            }],
            events: vec![],
        });

        graph
    }

    #[tokio::test]
    async fn test_worker() -> anyhow::Result<()> {
        let output_stream = OutputStream::new();

        let timers_invoker = TimersInvoker::default();
        let basic_invoker = BasicInvoker::with_output_stream(&output_stream).await;

        let mut func_lib = basic_invoker.into_func_lib();
        func_lib.merge(timers_invoker.into_func_lib());

        let graph = log_frame_no_graph();

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(move |result| {
            compute_finish_tx
                .try_send(result)
                .expect("Failed to send a compute callback event");
        });

        worker.run_once(graph.clone(), func_lib.clone()).await;
        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes, 3);
        assert_eq!(output_stream.take().await, ["1"]);

        worker.run_loop(graph.clone(), func_lib.clone()).await;
        worker.event().await;

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes, 3);
        assert_eq!(output_stream.take().await, ["2"]);

        worker.event().await;
        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes, 3);
        assert_eq!(output_stream.take().await, ["3"]);

        // worker.exit().await;

        Ok(())
    }
}
