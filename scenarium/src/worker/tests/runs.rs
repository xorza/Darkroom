use super::*;

/// The frame counter advances once per firing, and every firing runs the
/// whole subscribed cone.
#[tokio::test]
async fn each_frame_event_runs_the_subscribed_cone() {
    let mut w = TestWorker::frames();
    w.send_many([w.update(), w.fire("Frame Event", 0)]);

    for expected in ["1", "2", "3"] {
        if expected != "1" {
            w.send(w.fire("Frame Event", 0));
        }
        let run = w.run().await;
        assert_eq!(run.ran_node_count, 3);
        assert_eq!(run.logs(), [expected]);
    }
}

#[tokio::test]
async fn events_are_deduplicated() {
    let mut w = TestWorker::frames();
    w.send(w.update());

    let event = w.event("Frame Event", 0);
    w.send(WorkerMessage::Run {
        seeds: RunSeeds::events(vec![event, event, event]),
    });

    assert_eq!(w.run().await.logs(), ["1"]);
}

#[tokio::test]
async fn sink_seeds_run_the_sink() {
    let mut w = TestWorker::printing("hello");
    w.send_many([w.update(), TestWorker::sinks()]);

    let run = w.run().await;

    assert_eq!(run.ran_node_count, 1);
    assert_eq!(run.logs(), ["hello"]);
}

/// Node seeds end-to-end: the seed overrides a compiled disabled node and
/// the worker runs only its cone — the sink `Print` panics if reached.
#[tokio::test]
async fn node_seeds_override_a_disabled_node_and_run_only_its_cone() {
    let mut graph = TestGraph::sample();
    graph.never("mult");
    graph.never("Print");
    graph.cache_all(CacheMode::None);
    graph.disable("sum");
    let mut w = TestWorker::over(graph);
    let sum = w.id("sum");

    w.send_many([
        w.update(),
        WorkerMessage::Run {
            seeds: RunSeeds::nodes(vec![sum]),
        },
    ]);

    let run = w.run().await;
    assert_eq!(
        run.ran(),
        ["get_a", "get_b", "sum"],
        "only the disabled sum's cone ran"
    );
    assert!(
        w.graph.graph.find(sum).unwrap().disabled,
        "execution does not mutate the authoring graph"
    );
}

/// A compiled disabled sink must not participate in an ordinary sink run.
/// Every body panics if it executes.
#[tokio::test]
async fn a_disabled_sink_stays_out_of_sink_runs() {
    let mut graph = TestGraph::sample();
    graph.never_all();
    graph.disable("Print");
    let mut w = TestWorker::over(graph);

    w.send_many([w.update(), TestWorker::sinks()]);

    let run = w.run().await;
    assert_eq!(run.ran_node_count, 0);
    assert!(run.missing_inputs().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_with_no_active_run_does_not_reach_the_next_one() {
    let mut w = TestWorker::printing("hi");

    w.worker.request_cancel();
    w.send_many([w.update(), TestWorker::sinks()]);

    let run = w.run().await;
    assert!(!run.cancelled, "an idle cancel affected the next run");
    assert_eq!(run.ran_node_count, 1, "the run completed in full");
}

#[tokio::test]
async fn sync_fires_after_execution() {
    let mut w = TestWorker::frames();

    w.settle([w.update(), w.fire("Frame Event", 0)]).await;

    w.run().await;
}
