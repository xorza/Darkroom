use crate::event::EventId;
use crate::execution_graph::{ExecutionGraph, ExecutionStats, Result};
use crate::function::FuncLib;
use crate::graph::{Graph, NodeId};
use common::Shared;
use hashbrown::HashSet;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tracing::error;

#[derive(Debug)]
pub enum WorkerMessage {
    Exit,
    Event,
    Update { graph: Graph, func_lib: FuncLib },
    Clear,
}

type EventQueue = Shared<Vec<EventId>>;

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: UnboundedSender<WorkerMessage>,
}

impl Worker {
    pub fn new<Callback>(callback: Callback) -> Self
    where
        Callback: Fn(Result<ExecutionStats>) + Send + 'static,
    {
        let callback: Shared<Callback> = Shared::new(callback);
        let (tx, rx) = unbounded_channel::<WorkerMessage>();
        let thread_handle: JoinHandle<()> = tokio::spawn(async move {
            worker_loop(rx, callback).await;
        });

        Self {
            thread_handle: Some(thread_handle),
            tx,
        }
    }

    pub fn send(&mut self, msg: WorkerMessage) {
        self.tx.send(msg).unwrap();
    }

    pub fn update(&mut self, graph: Graph, func_lib: FuncLib) {
        self.send(WorkerMessage::Update { graph, func_lib });
    }

    pub fn exit(&mut self) {
        self.send(WorkerMessage::Exit);

        if let Some(_thread_handle) = self.thread_handle.take() {
            // thread_handle.await.expect("Worker thread failed to join");
        }
    }
    pub fn event(&mut self) {
        self.send(WorkerMessage::Event);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if self.thread_handle.is_some() {
            error!("Worker dropped while the thread is still running; call Worker::exit() first");
        }
    }
}

async fn worker_loop<Callback>(mut rx: UnboundedReceiver<WorkerMessage>, callback: Shared<Callback>)
where
    Callback: Fn(Result<ExecutionStats>) + Send + 'static,
{
    let mut execution_graph = ExecutionGraph::default();
    let mut msgs: Vec<WorkerMessage> = Vec::default();
    let mut context: Option<(Graph, FuncLib)> = None;
    let mut invalidate_node_ids: HashSet<NodeId> = HashSet::default();
    let mut events: Vec<u8> = Vec::default();

    'worker: loop {
        let msg = rx.recv().await;
        let Some(msg) = msg else { break };
        msgs.push(msg);

        loop {
            match rx.try_recv() {
                Ok(msg) => msgs.push(msg),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'worker,
            }
        }

        for msg in msgs.drain(..) {
            match msg {
                WorkerMessage::Exit => break 'worker,
                WorkerMessage::Event => events.push(0),
                WorkerMessage::Update { graph, func_lib } => {
                    events.clear();
                    context = Some((graph, func_lib));
                }
                WorkerMessage::Clear => {
                    execution_graph.clear();
                    events.clear();
                    context = None;
                    invalidate_node_ids.clear();
                }
            }
        }

        if !invalidate_node_ids.is_empty() {
            execution_graph.invalidate_recursively(invalidate_node_ids.drain());
            assert!(invalidate_node_ids.is_empty());
        }

        if let Some((graph, func_lib)) = context.take()
            && let Err(err) = execution_graph.update(&graph, &func_lib)
        {
            (callback.lock().await)(Err(err));
        }

        if !events.is_empty() {
            let result = execution_graph.execute().await;
            (callback.lock().await)(result);
            events.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use common::output_stream::OutputStream;

    use crate::elements::basic_invoker::BasicInvoker;
    use crate::elements::timers_invoker::TimersFuncLib;
    use crate::function::FuncId;
    use crate::graph::NodeId;
    use crate::graph::{Binding, Graph, Input, Node, NodeBehavior};

    use crate::worker::Worker;

    fn log_frame_no_graph() -> Graph {
        let mut graph = Graph::default();

        let frame_event_node_id: NodeId = "e69c3f32-ac66-4447-a3f6-9e8528c5d830".into();
        let frame_event_func_id: FuncId = "01897c92-d605-5f5a-7a21-627ed74824ff".into();

        let float_to_string_node_id: NodeId = "eb6590aa-229d-4874-abba-37c56f5b97fa".into();
        let float_to_string_func_id: FuncId = "01896a88-bf15-dead-4a15-5969da5a9e65".into();

        let print_node_id: NodeId = "8be72298-dece-4a5f-8a1d-d2dee1e791d3".into();
        let print_func_id: FuncId = "01896910-0790-ad1b-aa12-3f1437196789".into();

        graph.add(Node {
            id: frame_event_node_id,
            func_id: frame_event_func_id,
            name: "frame event".to_string(),
            behavior: NodeBehavior::AsFunction,
            terminal: false,
            inputs: vec![Input {
                binding: Binding::None,
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
            }],
            events: vec![],
        });

        graph
    }

    #[tokio::test]
    async fn test_worker() -> anyhow::Result<()> {
        let output_stream = OutputStream::new();

        let timers_invoker = TimersFuncLib::default();
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

        worker.update(graph.clone(), func_lib.clone());
        worker.event();

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes.len(), 3);
        assert_eq!(output_stream.take().await, ["1"]);

        worker.event();

        let executed = compute_finish_rx
            .recv()
            .await
            .expect("Missing compute completion")
            .expect("Unsuccessful compute");

        assert_eq!(executed.executed_nodes.len(), 3);
        assert_eq!(output_stream.take().await, ["2"]);

        worker.event();

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
}
