use crate::event::EventId;
use crate::execution_graph::{ExecutionGraph, ExecutionStats, Result};
use crate::function::FuncLib;
use crate::graph::Graph;
use common::Shared;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
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
}

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

    pub fn send(&mut self, msg: WorkerMessage) {
        self.tx.send(msg).unwrap();
    }
    pub fn execute_terminals(&mut self) {
        self.send(WorkerMessage::ExecuteTerminals);
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

    pub fn event(&mut self, event_id: EventId) {
        self.send(WorkerMessage::Event { event_id });
    }

    pub fn execute_events<T: IntoIterator<Item = EventId>>(&mut self, event_ids: T) {
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

async fn worker_loop<Callback>(
    mut rx: UnboundedReceiver<WorkerMessage>,
    _tx: UnboundedSender<WorkerMessage>,
    callback: Shared<Callback>,
) where
    Callback: Fn(Result<ExecutionStats>) + Send + 'static,
{
    // start_event_loop(tx);

    let mut execution_graph = ExecutionGraph::default();
    let mut msgs: Vec<WorkerMessage> = Vec::default();

    let mut context: Option<(Graph, FuncLib)>;
    let mut events: Vec<EventId> = Vec::default();
    let mut execute_terminals: bool;

    'worker: loop {
        execute_terminals = false;
        events.clear();
        context = None;

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
                WorkerMessage::Event { event_id } => events.push(event_id),
                WorkerMessage::Events { event_ids } => events.extend(event_ids),
                WorkerMessage::Update { graph, func_lib } => {
                    events.clear();
                    context = Some((graph, func_lib));
                }
                WorkerMessage::Clear => {
                    execution_graph.clear();
                }
                WorkerMessage::ExecuteTerminals => execute_terminals = true,
                WorkerMessage::StartEventLoop => todo!(),
                WorkerMessage::StopEventLoop => todo!(),
            }
        }

        if let Some((graph, func_lib)) = context.take() {
            execution_graph.update(&graph, &func_lib)
        }

        if execute_terminals || !events.is_empty() {
            let result = execution_graph
                .execute(execute_terminals, events.drain(..))
                .await;
            (callback.lock().await)(result);
        }
    }
}

fn start_event_loop(tx: UnboundedSender<WorkerMessage>) {
    let (_event_tx, mut event_rx) = unbounded_channel::<Vec<EventId>>();

    tokio::spawn({
        async move {
            loop {
                let events = event_rx.recv().await;

                if let Some(events) = events {
                    let resutl = tx.send(WorkerMessage::Events { event_ids: events });
                    if resutl.is_err() {
                        return;
                    }
                } else {
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use common::output_stream::OutputStream;

    use crate::elements::basic_funclib::BasicFuncLib;
    use crate::elements::timers_funclib::{FRAME_EVENT_FUNC_ID, TimersFuncLib};
    use crate::event::EventId;
    use crate::function::FuncId;
    use crate::graph::{Binding, Graph, Input, Node, NodeBehavior};
    use crate::graph::{Event, NodeId};

    use crate::worker::Worker;

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
}
