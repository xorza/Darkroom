use super::*;

/// The whole loop lifecycle end to end. The second run is the one an event
/// *lambda* drove — it can only have come from the firing, since the
/// bootstrap is already accounted for — so this covers the lambda reaching
/// the worker's execute path through its mailbox.
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
    assert_eq!(fired.executed_node_count, 3);
    assert_eq!(fired.logs().len(), 1);

    w.drain();
    w.settle([WorkerMessage::StopEventLoop]).await;
    w.drain();
    w.assert_no_run().await;
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
    w.assert_no_run().await;
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
            w.run().await.executed_node_count,
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
                    async move { notify.notified().await }
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
        node.sink()
            .lambda(async_lambda!(move |_| { counter = counter.clone() } => {
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

/// An event lambda's panic ends the worker with it, as a func lambda's does, even while a
/// sibling source posts once per batch, a post ready at every wake: `exit` hands the panic back
/// for the host to resume.
#[tokio::test]
async fn an_event_lambda_panic_ends_the_worker_with_it() {
    let mut w = TestWorker::frames();
    w.graph.edit_func("Frame Event", |func| {
        func.events[0].event_lambda =
            EventLambda::new(|_state| async { panic!("event loop stopped") });
        func.events[1].event_lambda = EventLambda::new(|_state| async {});
    });
    w.graph.subscribe("Frame Event", 1, "Print");

    w.send_many([w.update(), WorkerMessage::StartEventLoop]);
    w.ended().await;

    assert_panicked_with(w.worker.exit(PATIENCE).await, "event loop stopped");
    assert!(w.worker.send(WorkerMessage::Clear).is_err());
}

/// A graph op voids the events fired before it, which name the program it replaces: a fire of
/// `Frame Event` merged with an install of a graph that lacks it runs nothing, where the planner
/// would have looked for an emitter the program does not hold.
#[tokio::test]
async fn a_fire_before_an_update_that_drops_its_emitter_runs_nothing() {
    let mut w = TestWorker::frames();
    w.settle([w.update()]).await;
    let fire = w.fire("Frame Event", 0);
    w.graph.remove("Frame Event");

    w.settle_without_run([fire, w.update()]).await;
    w.sync().await;
}

/// A batch that stops the loop voids the events fired into it: the loop that fired them is gone.
#[tokio::test]
async fn a_stop_voids_the_events_fired_beside_it() {
    let mut w = TestWorker::frames();
    w.settle([w.update()]).await;

    w.settle_without_run([w.fire("Frame Event", 0), WorkerMessage::StopEventLoop])
        .await;
}
