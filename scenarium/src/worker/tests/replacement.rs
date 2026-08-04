use super::*;

/// A run seeded with an event the *replacing* program no longer holds is
/// refused, and the worker carries on with the new program.
#[tokio::test]
async fn a_stale_event_seed_is_rejected_without_stopping_the_worker() {
    let mut w = TestWorker::frames();
    w.settle([w.update()]).await;
    let stale = w.event("Frame Event", 0);

    w.graph = TestWorker::print_graph("replacement");
    w.send_many([
        w.update(),
        WorkerMessage::Run {
            seeds: RunSeeds::events(vec![stale]),
        },
    ]);

    let error = w.finished().await.expect_err("the stale seed is refused");
    assert!(
        matches!(error, Error::EventSeedNotFound { event } if event == stale),
        "unexpected stale-event error: {error:?}"
    );

    w.send(TestWorker::sinks());
    assert_eq!(w.run().await.logs(), ["replacement"]);
}

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
    graph.add("sink", |node| node.records());
    graph.wire("source", 0, "sink", 0);

    let mut w = TestWorker::over(graph);
    let source = w.id("source");
    let sink = w.id("sink");
    let running = w.compile();
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
    let replacement = w.compile();
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
