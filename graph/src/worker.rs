use std::sync::Arc;

use crate::compute::{Compute, ComputeResult};
use crate::event::EventId;
use crate::execution_graph::ExecutionGraph;
use crate::function::FuncLib;
use crate::graph::Graph;
use pollster::FutureExt;
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
    RunOnce(Graph),
    RunLoop(Graph),
}

type ComputeEvent = dyn Fn(ComputeResult<()>) + Send + 'static;

type EventQueue = Arc<Mutex<Vec<EventId>>>;

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: Sender<WorkerMessage>,
}

impl Worker {
    pub fn new<Callback>(func_lib: FuncLib, compute_callback: Callback) -> Self
    where
        Callback: Fn(ComputeResult<()>) + Send + 'static,
    {
        let compute_callback: Arc<Mutex<ComputeEvent>> = Arc::new(Mutex::new(compute_callback));

        let (tx, rx) = channel::<WorkerMessage>(10);
        let thread_handle: JoinHandle<()> = tokio::spawn(async move {
            Self::worker_loop(func_lib, rx, compute_callback).await;
        });

        Self {
            thread_handle: Some(thread_handle),
            tx,
        }
    }

    async fn worker_loop(
        func_lib: FuncLib,
        mut rx: Receiver<WorkerMessage>,
        compute_callback: Arc<Mutex<ComputeEvent>>,
    ) {
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

                WorkerMessage::RunOnce(graph) => {
                    let mut execution_graph = ExecutionGraph::default();
                    let result = Compute::default()
                        .run(&graph, &func_lib, &mut execution_graph)
                        .await;
                    (*compute_callback.lock().await)(result);
                }

                WorkerMessage::RunLoop(graph) => {
                    message =
                        Self::event_subloop(graph, &func_lib, &mut rx, compute_callback.clone())
                            .await;
                }
            }
        }
    }

    async fn event_subloop(
        graph: Graph,
        func_lib: &FuncLib,
        worker_rx: &mut Receiver<WorkerMessage>,
        compute_callback: Arc<Mutex<ComputeEvent>>,
    ) -> Option<WorkerMessage> {
        let mut execution_graph = ExecutionGraph::default();

        loop {
            // receive all messages and pick message with the highest priority
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

                WorkerMessage::Exit | WorkerMessage::RunOnce(_) | WorkerMessage::RunLoop(_) => {
                    break Some(msg);
                }

                WorkerMessage::Event => {
                    let result = Compute::default()
                        .run(&graph, func_lib, &mut execution_graph)
                        .await;

                    (*compute_callback.lock().await)(result);
                }
            }
        }
    }

    pub async fn run_once(&mut self, graph: Graph) {
        self.tx
            .send(WorkerMessage::RunOnce(graph))
            .await
            .expect("Failed to send run_once message");
    }
    pub async fn run_loop(&mut self, graph: Graph) {
        self.tx
            .send(WorkerMessage::RunLoop(graph))
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
            WorkerMessage::Stop => 127,
            WorkerMessage::RunOnce(_) | WorkerMessage::RunLoop(_) => 64,
            WorkerMessage::Event => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use common::output_stream::OutputStream;

    use crate::data::StaticValue;
    use crate::elements::basic_invoker::BasicInvoker;
    use crate::elements::timers_invoker::TimersInvoker;
    use crate::function::FuncId;
    use crate::graph::NodeId;
    use crate::graph::{Binding, Graph, Input, Node, NodeBehavior};
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
            behavior: NodeBehavior::OnInputChange,
            inputs: vec![Input {
                binding: Binding::Const,
                const_value: Some(StaticValue::Float(30.0)),
            }],
            events: vec![],
        });

        graph.add(Node {
            id: float_to_string_node_id,
            func_id: float_to_string_func_id,
            name: "float to string".to_string(),
            behavior: NodeBehavior::OnInputChange,
            inputs: vec![Input {
                binding: Binding::from_output_binding(frame_event_node_id, 1),
                const_value: None,
            }],
            events: vec![],
        });

        graph.add(Node {
            id: print_node_id,
            func_id: print_func_id,
            name: "print".to_string(),
            behavior: NodeBehavior::Terminal,
            inputs: vec![Input {
                binding: Binding::from_output_binding(float_to_string_node_id, 0),
                const_value: None,
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

        let (compute_finish_tx, mut compute_finish_rx) = tokio::sync::mpsc::channel(8);
        let mut worker = Worker::new(func_lib, move |result| {
            compute_finish_tx
                .try_send(result)
                .expect("Failed to send a compute callback event");
        });

        let graph = log_frame_no_graph();

        worker.run_once(graph.clone()).await;
        compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(output_stream.take().await[0], "1");

        worker.run_loop(graph.clone()).await;

        worker.event().await;
        compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        worker.event().await;
        compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        let log = output_stream.take().await;
        assert_eq!(log[0], "1");
        assert_eq!(log[1], "2");

        worker.exit().await;

        Ok(())
    }
}
