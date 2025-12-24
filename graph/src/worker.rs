use std::sync::Arc;

use pollster::FutureExt;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::compute::Compute;
use crate::event::EventId;
use crate::function::FuncLib;
use crate::graph::Graph;
use crate::invoke::{Invoker, UberInvoker};
use crate::runtime_graph::RuntimeGraph;

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
    pub fn new<Callback>(invoker: UberInvoker, compute_callback: Callback) -> Self
    where
        Callback: Fn() + Send + 'static,
    {
        let compute_callback: Arc<Mutex<ComputeEvent>> = Arc::new(Mutex::new(compute_callback));

        let (worker_tx, worker_rx) = channel::<WorkerMessage>(10);
        let thread_handle: JoinHandle<()> = tokio::spawn(async move {
            Self::worker_loop(invoker, worker_rx, compute_callback).await;
        });

        Self {
            thread_handle: Some(thread_handle),
            tx: worker_tx,
        }
    }

    async fn worker_loop(
        invoker: UberInvoker,
        mut rx: Receiver<WorkerMessage>,
        compute_callback: Arc<Mutex<ComputeEvent>>,
    ) {
        let mut message: Option<WorkerMessage> = None;
        let func_lib = invoker.get_func_lib();

        loop {
            if message.is_none() {
                message = rx.recv().await;
            }
            if message.is_none() {
                break;
            }

            match message.take().expect("Worker loop received empty message") {
                WorkerMessage::Stop | WorkerMessage::Event => panic!("Unexpected event message"),

                WorkerMessage::Exit => break,

                WorkerMessage::RunOnce(graph) => {
                    let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
                    Compute::default()
                        .run(&graph, &func_lib, &invoker, &mut runtime_graph)
                        .await
                        .expect("Failed to run graph");
                    (*compute_callback.lock().await)();
                }

                WorkerMessage::RunLoop(graph) => {
                    message = Self::event_subloop(
                        graph,
                        &func_lib,
                        &invoker,
                        &mut rx,
                        compute_callback.clone(),
                    )
                    .await;
                }
            }
        }
    }

    #[allow(unreachable_code)]
    async fn event_subloop(
        graph: Graph,
        func_lib: &FuncLib,
        invoker: &UberInvoker,
        worker_rx: &mut Receiver<WorkerMessage>,
        compute_callback: Arc<Mutex<ComputeEvent>>,
    ) -> Option<WorkerMessage> {
        let mut result_message: Option<WorkerMessage> = None;
        let mut runtime_graph = RuntimeGraph::new(&graph, func_lib);

        loop {
            // receive all messages and pick message with the highest priority
            let mut msg = worker_rx
                .recv()
                .await
                .expect("Worker message channel closed");
            while let Ok(another_msg) = worker_rx.try_recv() {
                // latest message has higher priority
                if another_msg.priority() >= msg.priority() {
                    msg = another_msg;
                }
            }

            match msg {
                WorkerMessage::Stop => break,

                WorkerMessage::Exit | WorkerMessage::RunOnce(_) | WorkerMessage::RunLoop(_) => {
                    result_message = Some(msg);
                    break;
                }

                WorkerMessage::Event => {
                    Compute::default()
                        .run(&graph, func_lib, invoker, &mut runtime_graph)
                        .await
                        .expect("Failed to run graph");

                    (*compute_callback.lock().await)();
                }
            }
        }

        result_message
    }

    pub fn run_once(&mut self, graph: Graph) {
        let msg = WorkerMessage::RunOnce(graph);

        self.tx
            .send(msg)
            .block_on()
            .expect("Failed to send run_once message");
    }
    pub fn run_loop(&mut self, graph: Graph) {
        let msg = WorkerMessage::RunLoop(graph);

        self.tx
            .send(msg)
            .block_on()
            .expect("Failed to send run_loop message");
    }
    pub fn stop(&mut self) {
        let msg = WorkerMessage::Stop;

        self.tx
            .send(msg)
            .block_on()
            .expect("Failed to send stop message");
    }

    #[cfg(test)]
    pub(crate) fn event(&mut self) {
        let msg = WorkerMessage::Event;

        self.tx
            .send(msg)
            .block_on()
            .expect("Failed to send event message");
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.tx
            .send(WorkerMessage::Exit)
            .block_on()
            .expect("Failed to send exit message");
        if let Some(thread_handle) = self.thread_handle.take() {
            thread_handle
                .block_on()
                .expect("Worker thread failed to join");
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
    use crate::invoke::UberInvoker;
    use crate::worker::Worker;

    #[test]
    fn test_worker() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let output_stream = OutputStream::new();

            let timers_invoker = TimersInvoker::default();
            let mut basic_invoker = BasicInvoker::default();
            basic_invoker.use_output_stream(&output_stream).await;

            let mut uber_invoker = UberInvoker::default();
            uber_invoker.merge(timers_invoker);
            uber_invoker.merge(basic_invoker);

            let (tx, rx) = mpsc::channel();
            let mut worker = Worker::new(uber_invoker, move || {
                tx.send(()).unwrap();
            });

            let graph = Graph::from_yaml_file("../test_resources/log_frame_no.yaml").unwrap();

            worker.run_once(graph.clone());
            rx.recv().unwrap();

            assert_eq!(output_stream.take().await[0], "1");

            worker.run_loop(graph.clone());

            worker.event();
            rx.recv().unwrap();

            worker.event();
            rx.recv().unwrap();

            let log = output_stream.take().await;
            assert_eq!(log[0], "1");
            assert_eq!(log[1], "2");

            worker.stop();
        });
    }
}
