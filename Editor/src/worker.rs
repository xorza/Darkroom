use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::thread;

use graph_lib::compute::Compute;
use graph_lib::elements::basic_invoker::BasicInvoker;
use graph_lib::graph::Graph;
use graph_lib::invoke::UberInvoker;
use graph_lib::runtime_graph::RuntimeGraph;

enum WorkerMessage {
    Stop,
    RunOnce(Graph),
    RunLoop(Graph),
}

#[derive(Debug)]
pub(crate) struct Worker {
    thread_handle: Option<thread::JoinHandle<()>>,
    sender: mpsc::Sender<WorkerMessage>,
}

impl Worker {
    pub(crate) fn new() -> Self {
        let (tx, rx) =
            mpsc::channel::<WorkerMessage>();

        let thread_handle =
            thread::spawn(move || { Self::worker_loop(rx); });

        Self {
            thread_handle: Some(thread_handle),
            sender: tx,
        }
    }

    fn worker_loop(rx: Receiver<WorkerMessage>) {
        let invoker = UberInvoker::new(
            vec![
                Box::new(BasicInvoker::default()),
            ]
        );
        let compute = Compute::from(invoker);

        let mut worker_message: Option<WorkerMessage> = None;
        loop {
            if worker_message.is_none() {
                worker_message = Some(rx.recv().unwrap());
            }

            match worker_message.take().as_ref().unwrap() {
                WorkerMessage::Stop => break,
                WorkerMessage::RunOnce(graph) => {
                    let mut runtime_graph = RuntimeGraph::from(graph);
                    compute.run(graph, &mut runtime_graph)
                        .expect("Failed to run graph");
                }
                WorkerMessage::RunLoop(graph) => {
                    let mut runtime_graph = RuntimeGraph::from(graph);
                    loop {
                        compute.run(graph, &mut runtime_graph)
                            .expect("Failed to run graph");

                        let new_msg = rx.try_recv().ok();
                        if new_msg.is_some() {
                            worker_message = new_msg;
                            break;
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn run_once(
        &mut self,
        graph: Graph,
    ) {
        let msg = WorkerMessage::RunOnce(graph);

        self.sender
            .send(msg)
            .unwrap();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.sender.send(WorkerMessage::Stop).unwrap();
        if let Some(thread_handle) = self.thread_handle.take() {
            thread_handle.join().unwrap();
        }
    }
}

