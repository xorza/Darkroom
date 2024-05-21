use std::sync::mpsc;

use common::log_setup::setup_logging;
use common::output_stream::OutputStream;

use crate::elements::basic_invoker::BasicInvoker;
use crate::elements::timers_invoker::TimersInvoker;
use crate::graph::Graph;
use crate::worker::Worker;

#[test]
fn test_worker() {
    setup_logging("debug");

    let (tx, rx) = mpsc::channel();
    let mut basic_invoker = Box::<BasicInvoker>::default();
    let output_stream = OutputStream::new();
    basic_invoker.use_output_stream(&output_stream);

    let mut worker = Worker::new(
        vec![basic_invoker, Box::<TimersInvoker>::default()],
        move || {
            tx.send(()).unwrap();
        },
    );

    let graph = Graph::from_yaml_file("../test_resources/log_frame_no.yaml").unwrap();

    worker.run_once(graph.clone());
    rx.recv().unwrap();

    assert_eq!(output_stream.take()[0], "1");

    worker.run_loop(graph.clone());

    worker.event();
    rx.recv().unwrap();

    worker.event();
    rx.recv().unwrap();

    let log = output_stream.take();
    assert_eq!(log[0], "1");
    assert_eq!(log[1], "2");

    worker.stop();
}
