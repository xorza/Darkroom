use std::cmp::Ordering;
use std::mem::swap;
use std::sync::{Arc, Condvar};
use std::thread;

use tokio::runtime::Runtime;

use crate::compute::Compute;
use crate::elements::basic_invoker::{BasicInvoker, Logger};
use crate::elements::timers_invoker::TimersInvoker;
use crate::event::EventId;
use crate::function::Function;
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

type EventQueue = Arc<tokio::sync::Mutex<Vec<EventId>>>;

#[derive(Debug)]
pub struct Worker {
    thread_handle: Option<thread::JoinHandle<()>>,
    tx: std::sync::mpsc::Sender<WorkerMessage>,
    all_functions: Vec<Function>,
    pub logger: Logger,
}

impl Worker {
    pub fn new<Callback>(
        compute_callback: Callback,
    ) -> Self
    where Callback: Fn() + Send + 'static
    {
        let (tx, rx) =
            std::sync::mpsc::channel::<WorkerMessage>();

        let on_all_function_ready_convar = Arc::new(Condvar::new());
        let all_functions: Arc<std::sync::Mutex<Vec<Function>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let on_all_function_ready_convar_clone = on_all_function_ready_convar.clone();
        let all_functions_clone = all_functions.clone();
        let tx_clone = tx.clone();

        let logger = Arc::new(std::sync::Mutex::new(Vec::new()));
        let logger_clone = logger.clone();

        let thread_handle =
            thread::spawn(move || {
                let invoker = UberInvoker::new(vec![
                    Box::new(BasicInvoker::new(logger_clone)),
                    Box::<TimersInvoker>::default(),
                ]);

                {
                    let mut all_functions_mutex = all_functions_clone.lock().unwrap();
                    let all_functions = invoker.all_functions();
                    *all_functions_mutex = all_functions;
                }

                let compute = Compute::from(invoker);

                on_all_function_ready_convar_clone.notify_all();

                let compute_callback = Arc::new(compute_callback);
                Self::worker_loop(compute, tx_clone, rx, compute_callback);
            });

        let mut all_functions_mutex = on_all_function_ready_convar
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
            tx,
            all_functions,
            logger,
        }
    }

    fn worker_loop(
        compute: Compute,
        tx: std::sync::mpsc::Sender<WorkerMessage>,
        rx: std::sync::mpsc::Receiver<WorkerMessage>,
        compute_callback: Arc<ComputeEvent>,
    ) {
        let mut message: Option<WorkerMessage> = None;

        loop {
            if message.is_none() {
                message = Some(rx.recv().unwrap());
            }

            match message.take().unwrap() {
                WorkerMessage::Stop |
                WorkerMessage::Event => panic!("Unexpected event message"),

                WorkerMessage::Exit => break,

                WorkerMessage::RunOnce(graph) => {
                    let mut runtime_graph = RuntimeGraph::from(&graph);
                    compute.run(&graph, &mut runtime_graph)
                        .expect("Failed to run graph");
                }

                WorkerMessage::RunLoop(graph) => {
                    message = Self::event_subloop(
                        graph,
                        &compute,
                        tx.clone(),
                        &rx,
                        &compute_callback,
                    );
                }
            }

            compute_callback();
        }
    }

    fn event_subloop(
        graph: Graph,
        compute: &Compute,
        worker_tx: std::sync::mpsc::Sender<WorkerMessage>,
        worker_rx: &std::sync::mpsc::Receiver<WorkerMessage>,
        compute_callback: &Arc<ComputeEvent>,
    ) -> Option<WorkerMessage>
    {
        let mut result_message: Option<WorkerMessage> = None;

        let runtime = Runtime::new().unwrap();
        let mut runtime_graph = RuntimeGraph::from(&graph);
        let event_queue: EventQueue = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let (_event_tx, mut event_rx) =
            tokio::sync::mpsc::channel::<EventId>(25);

        runtime.spawn(async move {
            loop {
                let event = event_rx.recv().await;
                if event.is_none() {
                    break;
                }

                let mut event_queue_mutex = event_queue.lock().await;
                event_queue_mutex.push(event.unwrap());

                let sent = worker_tx.send(WorkerMessage::Event);
                if sent.is_err() {
                    break;
                }
            }
        });

        loop {
            // receive all messages and pick message with highest priority
            let mut msg = worker_rx.recv().unwrap();
            while let Ok(another_msg) = worker_rx.try_recv() {
                if another_msg >= msg {
                    msg = another_msg;
                }
            }

            match msg {
                WorkerMessage::Stop => break,
                WorkerMessage::Exit |
                WorkerMessage::RunOnce(_) |
                WorkerMessage::RunLoop(_) => {
                    result_message = Some(msg);
                    break;
                }

                WorkerMessage::Event => {
                    compute
                        .run(&graph, &mut runtime_graph)
                        .expect("Failed to run graph");

                    compute_callback();
                    continue;
                },
            }

            #[allow(unreachable_code)]
            { unreachable!(); }
        }

        runtime.shutdown_background();

        result_message
    }

    pub fn run_once(
        &mut self,
        graph: Graph,
    ) {
        // let msg = WorkerMessage::RunOnce(graph);
        let msg = WorkerMessage::RunLoop(graph);

        self.tx
            .send(msg)
            .unwrap();
    }

    pub fn all_functions(&self) -> &Vec<Function> {
        &self.all_functions
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.tx.send(WorkerMessage::Exit).unwrap();
        if let Some(thread_handle) = self.thread_handle.take() {
            thread_handle.join().unwrap();
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
            (WorkerMessage::     Event, WorkerMessage::Event     ) => Some(Ordering::  Equal),
            (WorkerMessage::     Event, WorkerMessage::Exit      ) => Some(Ordering::   Less),
            (WorkerMessage::     Event, WorkerMessage::Stop      ) => Some(Ordering::   Less),
            (WorkerMessage::     Event, WorkerMessage::RunOnce(_)) => Some(Ordering::   Less),
            (WorkerMessage::     Event, WorkerMessage::RunLoop(_)) => Some(Ordering::   Less),

            (WorkerMessage::      Exit, WorkerMessage::Event     ) => Some(Ordering::Greater),
            (WorkerMessage::      Exit, WorkerMessage::Exit      ) => Some(Ordering::  Equal),
            (WorkerMessage::      Exit, WorkerMessage::Stop      ) => Some(Ordering::Greater),
            (WorkerMessage::      Exit, WorkerMessage::RunOnce(_)) => Some(Ordering::Greater),
            (WorkerMessage::      Exit, WorkerMessage::RunLoop(_)) => Some(Ordering::Greater),

            (WorkerMessage::      Stop, WorkerMessage::Event     ) => Some(Ordering::Greater),
            (WorkerMessage::      Stop, WorkerMessage::Exit      ) => Some(Ordering::   Less),
            (WorkerMessage::      Stop, WorkerMessage::Stop      ) => Some(Ordering::  Equal),
            (WorkerMessage::      Stop, WorkerMessage::RunOnce(_)) => Some(Ordering::   Less),
            (WorkerMessage::      Stop, WorkerMessage::RunLoop(_)) => Some(Ordering::   Less),

            (WorkerMessage::RunOnce(_), WorkerMessage::Event     ) => Some(Ordering::Greater),
            (WorkerMessage::RunOnce(_), WorkerMessage::Exit      ) => Some(Ordering::   Less),
            (WorkerMessage::RunOnce(_), WorkerMessage::Stop      ) => Some(Ordering::Greater),
            (WorkerMessage::RunOnce(_), WorkerMessage::RunOnce(_)) => None                   ,
            (WorkerMessage::RunOnce(_), WorkerMessage::RunLoop(_)) => None                   ,

            (WorkerMessage::RunLoop(_), WorkerMessage::Event     ) => Some(Ordering::Greater),
            (WorkerMessage::RunLoop(_), WorkerMessage::Exit      ) => Some(Ordering::   Less),
            (WorkerMessage::RunLoop(_), WorkerMessage::Stop      ) => Some(Ordering::Greater),
            (WorkerMessage::RunLoop(_), WorkerMessage::RunOnce(_)) => None                   ,
            (WorkerMessage::RunLoop(_), WorkerMessage::RunLoop(_)) => None                   ,
            // @formatter:on
        }
    }
}