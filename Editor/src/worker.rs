use std::mem::swap;
use std::sync::{Arc, Condvar, mpsc, Mutex};
use std::sync::mpsc::Receiver;
use std::thread;

use eframe::egui;

use graph_lib::compute::Compute;
use graph_lib::elements::basic_invoker::{BasicInvoker, Logger};
use graph_lib::function::Function;
use graph_lib::graph::Graph;
use graph_lib::invoke::{Invoker, UberInvoker};
use graph_lib::runtime_graph::RuntimeGraph;

use crate::timers_invoker::TimersInvoker;

enum WorkerMessage {
    Stop,
    RunOnce(Graph),
}


#[derive(Debug)]
pub(crate) struct Worker {
    thread_handle: Option<thread::JoinHandle<()>>,
    sender: mpsc::Sender<WorkerMessage>,
    all_functions: Vec<Function>,
}

impl Worker {
    pub(crate) fn new(
        egui_ctx: egui::Context,
        logger: Logger,
    ) -> Self {
        let (tx, rx) =
            mpsc::channel::<WorkerMessage>();

        let condvar = Arc::new(Condvar::new());
        let all_functions: Arc<Mutex<Vec<Function>>> = Arc::new(Mutex::new(Vec::new()));

        let condvar_clone = condvar.clone();
        let all_functions_clone = all_functions.clone();

        let thread_handle =
            thread::spawn(move || {
                let invoker = UberInvoker::new(vec![
                    Box::new(BasicInvoker::new(logger)),
                    Box::new(TimersInvoker::default()),
                ]);

                {
                    let mut all_functions_mutex = all_functions_clone.lock().unwrap();
                    let all_functions = invoker.all_functions();
                    *all_functions_mutex = all_functions;
                }

                let compute = Compute::from(invoker);

                condvar_clone.notify_all();

                Self::worker_loop(compute, rx, egui_ctx);
            });

        let mut all_functions_mutex = condvar
            .wait_while(
                all_functions.lock().unwrap(),
                |functions| {
                    functions.is_empty()
                })
            .unwrap();

        let mut all_functions = vec![];
        swap(&mut *all_functions_mutex, &mut all_functions);

        Self {
            thread_handle: Some(thread_handle),
            sender: tx,
            all_functions,
        }
    }

    fn worker_loop(
        compute: Compute,
        rx: Receiver<WorkerMessage>,
        egui_ctx: egui::Context,
    ) {
        let mut worker_message: Option<WorkerMessage> = None;
        loop {
            if worker_message.is_none() {
                worker_message = Some(rx.recv().unwrap());
            }

            match worker_message.take().unwrap() {
                WorkerMessage::Stop => break,
                WorkerMessage::RunOnce(graph) => {
                    let mut runtime_graph = RuntimeGraph::from(&graph);
                    compute.run(&graph, &mut runtime_graph)
                        .expect("Failed to run graph");
                }
            }

            egui_ctx.request_repaint();
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

    pub(crate) fn all_functions(&self) -> &Vec<Function> {
        &self.all_functions
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

