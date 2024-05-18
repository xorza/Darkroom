use std::sync::mpsc;

use common::log_setup::setup_logging;

use crate::elements::basic_invoker::BasicInvoker;
use crate::elements::timers_invoker::TimersInvoker;
use crate::graph::Graph;
use crate::worker::Worker;

#[test]
fn test_worker() {
    setup_logging("debug");

    let (tx, rx) = mpsc::channel();

    let mut worker = Worker::new(
        move |logger| {
            vec![
                Box::new(BasicInvoker::new(logger)),
                Box::<TimersInvoker>::default(),
            ]
        },
        move || {
            tx.send(()).unwrap();
        },
    );

    let graph = Graph::from_yaml_file("../test_resources/log_frame_no.yaml").unwrap();

    worker.run_once(graph.clone());
    rx.recv().unwrap();
    {
        let logger = worker.logger.lock().unwrap();
        assert_eq!(logger[0], "1");
    }

    worker.run_loop(graph.clone());

    worker.event();
    rx.recv().unwrap();

    worker.event();
    rx.recv().unwrap();

    {
        let logger = worker.logger.lock().unwrap();
        assert_eq!(logger[1], "1");
        assert_eq!(logger[2], "2");
    }

    worker.stop();
}
