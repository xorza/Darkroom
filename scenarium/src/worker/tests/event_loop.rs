use super::*;

/// The whole loop lifecycle end to end. The second run is the one an event
/// *lambda* drove — it can only have come from the firing, since the
/// bootstrap is already accounted for — so this covers the lambda reaching
/// the worker's execute path over the dedicated bounded channel.
///
/// The lambda then fires as fast as it can, and `settle` sends
/// `StopEventLoop` and waits for its `Sync` to come back: the command is
/// observed within a bounded time rather than starved by the event stream.
#[tokio::test]
async fn start_then_stop() {
    let mut w = TestWorker::frames();

    w.settle([w.update(), WorkerMessage::StartEventLoop]).await;

    // The bootstrap run initializes the event source and logs nothing; the
    // loop's first firing then runs the whole cone.
    assert!(w.run().await.logs().is_empty());
    let fired = w.run().await;
    assert_eq!(fired.ran_node_count, 3);
    assert_eq!(fired.logs().len(), 1);

    w.drain();
    w.settle([WorkerMessage::StopEventLoop]).await;
    w.drain();
    w.nothing_runs_within(QUIET).await;
}

#[tokio::test]
async fn stopping_a_loop_that_is_not_running_is_a_noop() {
    let w = TestWorker::over(TestGraph::new());

    w.settle([WorkerMessage::StopEventLoop]).await;
}

/// A batch triggering both an execution and `StartEventLoop` must run
/// execute once and publish one completion, not two. A sink-only graph
/// prepares no event triggers, so the loop never actually starts — which
/// removes lambda-driven runs as a confounding factor while still
/// exercising the rebuild transition.
#[tokio::test]
async fn sink_seeds_with_start_event_loop_complete_once() {
    let mut w = TestWorker::printing("hi");

    w.send_many([
        w.update(),
        TestWorker::sinks(),
        WorkerMessage::StartEventLoop,
    ]);

    assert_eq!(w.run().await.logs(), ["hi"]);
    w.nothing_runs_within(QUIET).await;
}

/// Either message arriving while the loop is already running stops the
/// current one and rebuilds it, rather than panicking or leaking it: an
/// `Update` restarts the loop over the new program, and a second
/// `StartEventLoop` is idempotent. Both must leave a loop that still fires.
#[tokio::test]
async fn update_or_a_second_start_rebuilds_a_running_loop() {
    for by_update in [false, true] {
        let mut w = TestWorker::frames();

        w.settle([w.update(), WorkerMessage::StartEventLoop]).await;
        w.drain();
        w.run().await;

        let restart = if by_update {
            w.update()
        } else {
            WorkerMessage::StartEventLoop
        };
        w.settle([restart]).await;
        w.drain();
        assert_eq!(
            w.run().await.ran_node_count,
            3,
            "the loop rebuilt by_update={by_update} still runs the whole cone"
        );
    }
}

/// A firing runs only the subscriber, never re-initializing the event
/// sources: `source_b`'s own lambda must not be invoked again because
/// `source_a` ticked.
#[tokio::test]
async fn a_fired_event_does_not_reinitialize_the_event_sources() {
    let (a_calls, b_calls, subscriber_calls) =
        (Calls::default(), Calls::default(), Calls::default());
    let a_notify = Arc::new(Notify::new());
    let b_notify = Arc::new(Notify::new());

    let mut graph = TestGraph::new();
    for (name, notify, counter) in [
        ("source_a", &a_notify, &a_calls),
        ("source_b", &b_notify, &b_calls),
    ] {
        let notify = Arc::clone(notify);
        let counter = counter.clone();
        graph.add(name, move |node| {
            node.event(
                "tick",
                EventLambda::new(move |_state| {
                    let notify = Arc::clone(&notify);
                    Box::pin(async move { notify.notified().await })
                }),
            )
            .lambda(async_lambda!(move |_| { counter = counter.clone() } => {
                counter.bump();
                Ok(())
            }))
        });
    }
    let counter = subscriber_calls.clone();
    graph.add("subscriber", move |node| {
        node.lambda(async_lambda!(move |_| { counter = counter.clone() } => {
            counter.bump();
            Ok(())
        }))
    });
    graph.subscribe("source_a", 0, "subscriber");
    graph.subscribe("source_b", 0, "subscriber");

    let mut w = TestWorker::over(graph);
    w.send_many([w.update(), WorkerMessage::StartEventLoop]);

    let bootstrap = w.run().await;
    assert_eq!(bootstrap.ran(), ["source_a", "source_b"]);
    assert_eq!(subscriber_calls.count(), 0);

    a_notify.notify_one();
    let fired = w.run().await;
    assert_eq!(fired.ran(), ["subscriber"], "only the subscriber re-ran");
    assert_eq!(a_calls.count(), 1);
    assert_eq!(b_calls.count(), 1);
    assert_eq!(subscriber_calls.count(), 1);

    w.settle([WorkerMessage::StopEventLoop]).await;
}

/// One event task panicking stops the whole loop and is attributed to its
/// node, even while a sibling task is still parked.
#[tokio::test]
async fn one_task_panicking_stops_the_loop() {
    let mut w = TestWorker::frames();
    w.graph.edit_func("Frame Event", |func| {
        func.events[0].event_lambda =
            EventLambda::new(|_state| Box::pin(async { panic!("event loop stopped") }));
        func.events[1].event_lambda = EventLambda::new(|_state| Box::pin(std::future::pending()));
    });
    w.graph.subscribe("Frame Event", 1, "Print");
    let frame_event = w.id("Frame Event");

    w.send_many([w.update(), WorkerMessage::StartEventLoop]);

    let mut activities = Vec::new();
    loop {
        match w.report().await {
            WorkerReport::Status(status) => {
                if activities.last() != Some(&status.activity) {
                    activities.push(status.activity);
                }
            }
            WorkerReport::Error(WorkerError::Execution {
                error: Error::EventLambdaPanic { node_id, message },
            }) => {
                assert_eq!(node_id, frame_event);
                assert!(message.contains("event loop stopped"));
                break;
            }
            WorkerReport::Installed(_)
            | WorkerReport::Cleared
            | WorkerReport::Error(WorkerError::Execution { .. })
            | WorkerReport::Error(WorkerError::CacheEviction { .. }) => {}
        }
    }
    assert_eq!(
        activities,
        [
            WorkerActivity::Executing,
            WorkerActivity::EventLoop,
            WorkerActivity::Idle,
        ]
    );
    w.sync().await;
}
