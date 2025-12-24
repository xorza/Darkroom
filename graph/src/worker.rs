use std::cmp::Ordering;
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
        let worker_tx_clone = worker_tx.clone();
        let thread_handle: JoinHandle<()> = tokio::spawn(async move {
            Self::worker_loop(
                Arc::new(invoker),
                worker_tx_clone,
                worker_rx,
                compute_callback,
            )
            .await;
        });

        Self {
            thread_handle: Some(thread_handle),
            tx: worker_tx,
        }
    }

    async fn worker_loop(
        invoker: Arc<UberInvoker>,
        tx: Sender<WorkerMessage>,
        mut rx: Receiver<WorkerMessage>,
        compute_callback: Arc<Mutex<ComputeEvent>>,
    ) {
        let mut message: Option<WorkerMessage> = None;
        let func_lib = Arc::new(invoker.get_func_lib());

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
                    let invoker = Arc::clone(&invoker);
                    let func_lib = Arc::clone(&func_lib);
                    tokio::task::spawn_blocking(move || {
                        let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
                        Compute::default()
                            .run(&graph, &func_lib, invoker.as_ref(), &mut runtime_graph)
                            .expect("Failed to run graph");
                    })
                    .await
                    .expect("RunOnce compute task panicked");
                }

                WorkerMessage::RunLoop(graph) => {
                    message = Self::event_subloop(
                        Arc::new(graph),
                        Arc::clone(&func_lib),
                        Arc::clone(&invoker),
                        tx.clone(),
                        &mut rx,
                        compute_callback.clone(),
                    )
                    .await;
                }
            }

            (*compute_callback.lock().await)();
        }
    }

    #[allow(unreachable_code)]
    async fn event_subloop(
        graph: Arc<Graph>,
        func_lib: Arc<FuncLib>,
        invoker: Arc<UberInvoker>,
        worker_tx: Sender<WorkerMessage>,
        worker_rx: &mut Receiver<WorkerMessage>,
        compute_callback: Arc<Mutex<ComputeEvent>>,
    ) -> Option<WorkerMessage> {
        let mut result_message: Option<WorkerMessage> = None;

        Self::start_event_thread(worker_tx);

        let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);

        loop {
            // receive all messages and pick message with the highest priority
            let mut msg = worker_rx
                .recv()
                .await
                .expect("Worker message channel closed");
            while let Ok(another_msg) = worker_rx.try_recv() {
                if another_msg >= msg {
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
                    let invoker = Arc::clone(&invoker);
                    let func_lib = Arc::clone(&func_lib);
                    let graph = Arc::clone(&graph);
                    runtime_graph = tokio::task::spawn_blocking(move || {
                        let mut runtime_graph = runtime_graph;
                        Compute::default()
                            .run(&graph, &func_lib, invoker.as_ref(), &mut runtime_graph)
                            .expect("Failed to run graph");
                        runtime_graph
                    })
                    .await
                    .expect("Event compute task panicked");

                    (*compute_callback.lock().await)();
                    continue;
                }
            }

            unreachable!();
        }

        result_message
    }

    fn start_event_thread(worker_tx: Sender<WorkerMessage>) {
        // fire events here

        // do not remove this, will be used in the future
        let event_queue: EventQueue = Arc::new(Mutex::new(Vec::new()));

        let (_event_tx, mut event_rx) = channel::<EventId>(25);

        tokio::spawn(async move {
            loop {
                let event = event_rx.recv().await;
                if event.is_none() {
                    break;
                }

                let mut event_queue_mutex = event_queue.lock().await;
                event_queue_mutex.push(event.expect("Event channel closed"));

                let sent = worker_tx.send(WorkerMessage::Event).await;
                if sent.is_err() {
                    break;
                }
            }
        });
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

impl PartialEq<Self> for WorkerMessage {
    fn eq(&self, other: &Self) -> bool {
        self.partial_cmp(other)
            .map(|ordering| ordering == Ordering::Equal)
            .unwrap_or(false)
    }
}

impl PartialOrd<Self> for WorkerMessage {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            // @formatter:off
            (WorkerMessage::Event, WorkerMessage::Event) => Some(Ordering::Equal),
            (WorkerMessage::Event, WorkerMessage::Exit) => Some(Ordering::Less),
            (WorkerMessage::Event, WorkerMessage::Stop) => Some(Ordering::Less),
            (WorkerMessage::Event, WorkerMessage::RunOnce(_)) => Some(Ordering::Less),
            (WorkerMessage::Event, WorkerMessage::RunLoop(_)) => Some(Ordering::Less),

            (WorkerMessage::Exit, WorkerMessage::Event) => Some(Ordering::Greater),
            (WorkerMessage::Exit, WorkerMessage::Exit) => Some(Ordering::Equal),
            (WorkerMessage::Exit, WorkerMessage::Stop) => Some(Ordering::Greater),
            (WorkerMessage::Exit, WorkerMessage::RunOnce(_)) => Some(Ordering::Greater),
            (WorkerMessage::Exit, WorkerMessage::RunLoop(_)) => Some(Ordering::Greater),

            (WorkerMessage::Stop, WorkerMessage::Event) => Some(Ordering::Greater),
            (WorkerMessage::Stop, WorkerMessage::Exit) => Some(Ordering::Less),
            (WorkerMessage::Stop, WorkerMessage::Stop) => Some(Ordering::Equal),
            (WorkerMessage::Stop, WorkerMessage::RunOnce(_)) => Some(Ordering::Less),
            (WorkerMessage::Stop, WorkerMessage::RunLoop(_)) => Some(Ordering::Less),

            (WorkerMessage::RunOnce(_), WorkerMessage::Event) => Some(Ordering::Greater),
            (WorkerMessage::RunOnce(_), WorkerMessage::Exit) => Some(Ordering::Less),
            (WorkerMessage::RunOnce(_), WorkerMessage::Stop) => Some(Ordering::Greater),
            (WorkerMessage::RunOnce(_), WorkerMessage::RunOnce(_)) => None,
            (WorkerMessage::RunOnce(_), WorkerMessage::RunLoop(_)) => None,

            (WorkerMessage::RunLoop(_), WorkerMessage::Event) => Some(Ordering::Greater),
            (WorkerMessage::RunLoop(_), WorkerMessage::Exit) => Some(Ordering::Less),
            (WorkerMessage::RunLoop(_), WorkerMessage::Stop) => Some(Ordering::Greater),
            (WorkerMessage::RunLoop(_), WorkerMessage::RunOnce(_)) => None,
            (WorkerMessage::RunLoop(_), WorkerMessage::RunLoop(_)) => None,
            // @formatter:on
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
