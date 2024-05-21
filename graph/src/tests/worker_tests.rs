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
    let mut basic_invoker = Box::<BasicInvoker>::default();
    let logger = basic_invoker.init_logger();

    let mut worker = Worker::new(
        move || vec![basic_invoker, Box::<TimersInvoker>::default()],
        move || {
            tx.send(()).unwrap();
        },
    );

    let graph = Graph::from_yaml_file("../test_resources/log_frame_no.yaml").unwrap();

    worker.run_once(graph.clone());
    rx.recv().unwrap();

    assert_eq!(logger.take_log(), "1");

    worker.run_loop(graph.clone());

    worker.event();
    rx.recv().unwrap();

    worker.event();
    rx.recv().unwrap();

    let log = logger.take_log();
    let mut log = log.lines();
    assert_eq!(log.next(), Some("1"));
    assert_eq!(log.next(), Some("2"));

    worker.stop();
}
