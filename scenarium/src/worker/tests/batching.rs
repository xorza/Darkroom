use super::*;

#[tokio::test]
async fn clear_resets_the_execution_graph() {
    let mut w = TestWorker::frames();
    w.send_many([w.update(), w.fire("Frame Event", 0)]);
    assert_eq!(w.run().await.logs(), ["1"]);

    w.send_many([WorkerMessage::Clear, w.fire("Frame Event", 0)]);

    // After Clear the frame event has no subscribers, so nothing runs.
    w.nothing_runs_within(QUIET).await;
}

/// Scan-then-commit ordering: `Clear` zeroes the execution graph, `Update`
/// queues a replacement, and the commit phase applies the update — so the
/// event still executes.
#[tokio::test]
async fn clear_then_update_in_one_batch_applies_the_update() {
    let mut w = TestWorker::frames();

    w.send_many([WorkerMessage::Clear, w.update(), w.fire("Frame Event", 0)]);

    let run = w.run().await;
    assert_eq!(run.ran_node_count, 3);
    assert_eq!(run.logs(), ["1"]);
}

#[tokio::test]
async fn update_then_clear_in_one_batch_leaves_the_graph_cleared() {
    let mut w = TestWorker::frames();

    w.send_many([w.update(), WorkerMessage::Clear, w.fire("Frame Event", 0)]);

    w.nothing_runs_within(QUIET).await;
}

/// An empty batch must not panic, hang, or desynchronize the worker.
#[tokio::test]
async fn an_empty_batch_is_a_noop() {
    let w = TestWorker::over(TestGraph::new());

    w.send_many(std::iter::empty::<WorkerMessage>());

    // A subsequent Sync still fires, so the worker is alive.
    w.sync().await;
}

#[tokio::test]
async fn every_sync_in_a_batch_fires() {
    let w = TestWorker::over(TestGraph::new());
    let (reply_a, ack_a) = oneshot::channel();
    let (reply_b, ack_b) = oneshot::channel();

    w.send_many([
        WorkerMessage::Sync { reply: reply_a },
        WorkerMessage::Sync { reply: reply_b },
    ]);

    for (ack, which) in [(ack_a, "first"), (ack_b, "second")] {
        timeout(Duration::from_millis(500), ack)
            .await
            .unwrap_or_else(|_| panic!("the {which} Sync never fired"))
            .unwrap_or_else(|_| panic!("the {which} sender was dropped"));
    }
}

/// Messages that become ready at the same wake reduce into one batch, so
/// the superseded update never runs.
#[tokio::test(flavor = "current_thread")]
async fn messages_ready_at_one_wake_reduce_together() {
    let mut w = TestWorker::printing("first");
    let first = w.compile();
    w.send(WorkerMessage::Update {
        compiled: Arc::clone(&first),
    });
    w.send(TestWorker::sinks());

    w.graph = TestWorker::print_graph("second");
    let second = w.compile();
    let second_print = w.id("Print");
    w.send(WorkerMessage::Update {
        compiled: Arc::clone(&second),
    });
    w.send(TestWorker::sinks());

    let run = w.run().await;
    assert!(!Arc::ptr_eq(w.installed(), &first));
    assert!(Arc::ptr_eq(w.installed(), &second));
    assert!(w.installed().contains(second_print));
    assert_eq!(run.logs(), ["second"]);
    w.nothing_runs_within(QUIET).await;
}
