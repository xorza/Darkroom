use super::*;
use crate::internals::test_graph::NodeSpec;

/// An `Update` arriving mid-run is installed only after the running program
/// reports — so the completion still describes the program that ran.
#[tokio::test(flavor = "multi_thread")]
async fn a_replacement_queued_mid_run_is_installed_after_the_running_program() {
    use std::sync::{Condvar, Mutex};

    let started = Arc::new(Notify::new());
    let release = Arc::new((Mutex::new(false), Condvar::new()));

    let mut graph = TestGraph::new();
    graph.add("source", |node| {
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        node.output(DataType::Int).compute(move |_| {
            started.notify_one();
            let (lock, wake) = &*release;
            let held = lock.lock().unwrap();
            drop(wake.wait_while(held, |released| !*released).unwrap());
            ConstValue::Int(7)
        })
    });
    graph.add("sink", NodeSpec::records);
    graph.wire("source", "sink");

    let mut w = TestWorker::over(graph);
    let source = w.id("source");
    let sink = w.id("sink");
    let running = w.graph.program();
    w.send_many([
        WorkerMessage::Update {
            compiled: Arc::clone(&running),
        },
        TestWorker::sinks(),
    ]);
    timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("the run did not start");

    w.graph = TestWorker::print_graph("next");
    let replacement = w.graph.program();
    let replacement_node = w.id("Print");
    w.send(WorkerMessage::Update {
        compiled: Arc::clone(&replacement),
    });
    {
        let (lock, wake) = &*release;
        *lock.lock().unwrap() = true;
        wake.notify_one();
    }
    w.sync().await;

    w.finished().await.expect("the running program completed");
    assert!(Arc::ptr_eq(w.installed(), &running));
    assert!(w.installed().contains(source));
    assert!(w.installed().contains(sink));
    assert!(!w.installed().contains(replacement_node));

    let WorkerReport::Installed {
        compiled: installed,
        ..
    } = w.report().await
    else {
        panic!("the replacement installed before the running program finished");
    };
    assert!(Arc::ptr_eq(&installed, &replacement));
}
