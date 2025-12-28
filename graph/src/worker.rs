use std::sync::Arc;

use crate::compute::Compute;
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

type ComputeEvent = dyn Fn() + Send + 'static;

type EventQueue = Arc<Mutex<Vec<EventId>>>;

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<JoinHandle<()>>,
    tx: Sender<WorkerMessage>,
}

impl Worker {
    pub fn new<Callback>(func_lib: FuncLib, compute_callback: Callback) -> Self
    where
        Callback: Fn() + Send + 'static,
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
                    (*compute_callback.lock().await)();
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

                    (*compute_callback.lock().await)();
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
    use std::sync::mpsc;

    use common::output_stream::OutputStream;

    use crate::elements::basic_invoker::BasicInvoker;
    use crate::elements::timers_invoker::TimersInvoker;
    use crate::graph::Graph;
    use crate::worker::Worker;

    #[test]
    fn test_worker() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let output_stream = OutputStream::new();

            let timers_invoker = TimersInvoker::default();
            let basic_invoker = BasicInvoker::with_output_stream(&output_stream).await;

            let mut func_lib = basic_invoker.into_func_lib();
            func_lib.merge(timers_invoker.into_func_lib());

            let (compute_finish_tx, compute_finish_rx) = mpsc::channel();
            let mut worker = Worker::new(func_lib, move || {
                compute_finish_tx
                    .send(())
                    .expect("Failed to send a compute callback event");
            });

            let graph = Graph::from_file("../test_resources/log_frame_no.yaml").unwrap();

            worker.run_once(graph.clone()).await;
            compute_finish_rx.recv().unwrap();

            assert_eq!(output_stream.take().await[0], "1");

            worker.run_loop(graph.clone()).await;

            worker.event().await;
            compute_finish_rx.recv().unwrap();

            worker.event().await;
            compute_finish_rx.recv().unwrap();

            let log = output_stream.take().await;
            assert_eq!(log[0], "1");
            assert_eq!(log[1], "2");

            worker.exit().await;
        });
    }
}
