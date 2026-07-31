use super::*;

use crate::async_lambda;
use crate::graph::func::event::EventLambda;
use crate::graph::node::special::SpecialNode;
use crate::testing::calls::Calls;

/// An impure source carrying a `tick` event and emitting its own call count —
/// so one logged value says both that it ran and how many times it has.
fn emitter(calls: &Calls) -> impl FnOnce(NodeSpec) -> NodeSpec {
    let body = calls.tally();
    move |n: NodeSpec| {
        n.output(DataType::Int)
            .event("tick", EventLambda::new(|_state| Box::pin(async move {})))
            .compute(body)
    }
}

/// `emit`: impure source with an output and one `tick` event, subscribed to
/// by `recv`. `recv`: impure consumer bound to emit's output. Neither is a
/// sink, so only event-driven execution reaches them.
fn event_pair() -> (TestGraph, Calls) {
    let calls = Calls::default();
    let mut g = TestGraph::new();
    g.add("emit", emitter(&calls));
    g.add("recv", |n| n.records());
    g.subscribe("emit", 0, "recv");
    g.wire("emit", 0, "recv", 0);
    (g, calls)
}

#[tokio::test(flavor = "multi_thread")]
async fn execute_events_runs_subscribers() -> TestResult {
    let (g, calls) = event_pair();
    let mut e = TestEngine::over(g);

    let tick = e.event("emit", 0);
    let run = e.run_events([tick]).await;

    // recv subscribes to emit's tick, so recv is the root and emit runs as
    // its dependency.
    assert_eq!(run.ran(), ["emit", "recv"]);
    assert_eq!(calls.count(), 1);
    assert_eq!(run.logs(), ["1"]);
    assert_eq!(
        run.triggered_events,
        [tick],
        "the triggering event is echoed back"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn event_sources_collects_nodes_with_subscribers() -> TestResult {
    let (g, calls) = event_pair();
    let mut e = TestEngine::over(g);

    // sinks=false, event_sources=true → emit (which owns a subscribed
    // event) becomes a root; recv is downstream of emit, not a root.
    let run = e.run_event_sources().await;

    assert_eq!(run.ran(), ["emit"]);
    assert_eq!(calls.count(), 1);
    assert!(run.logs().is_empty(), "recv is not reached");
    Ok(())
}

/// The bootstrap run re-initializes its event sources every time, bypassing
/// the cache — the shared state its event lambdas read has to be freshly
/// built even when the node's digest is unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_prepares_events_and_bypasses_source_cache() -> TestResult {
    let (mut g, calls) = event_pair();
    g.edit_func("emit", |func| func.behavior = FuncBehavior::Pure);
    g.cache("emit", CacheMode::Ram);
    let mut e = TestEngine::over(g);

    let tick = e.event("emit", 0);
    for expected in [1, 2] {
        let run = e.run_event_sources().await;
        assert_eq!(calls.count(), expected, "a pure, cached source re-runs");
        assert_eq!(run.armed_events, [tick]);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_prepares_no_events_without_subscribers() -> TestResult {
    let (mut g, _) = event_pair();
    // Drop the subscriber but keep emit reachable by making it a sink.
    g.unsubscribe("emit", 0, "recv");
    g.edit_func("emit", |func| func.sink = true);
    let mut e = TestEngine::over(g);

    let run = e.run_sinks().await;

    assert!(run.ran().contains(&"emit"));
    assert!(
        run.armed_events.is_empty(),
        "emit's event has no subscribers, so nothing is armed"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_event_source_prepares_no_trigger() -> TestResult {
    let (mut g, _) = event_pair();
    g.edit_func("emit", |func| {
        func.lambda = async_lambda!(|_| {
            Err(InvokeError::external(std::io::Error::other(
                "bootstrap failed",
            )))
        })
    });
    let mut e = TestEngine::over(g);

    let run = e.run_event_sources().await;

    assert_eq!(run.errored(), ["emit"]);
    assert!(run.armed_events.is_empty());
    Ok(())
}

/// A `RunSinks` special node subscribed to an event fires no cone of its own
/// (it has no ports) — instead firing that event runs *every* sink, exactly
/// as pressing "Run" would. Here `emit`'s tick reaches only the `RunSinks`
/// sink, yet the independent `source → sink` cone runs, while `emit`
/// (neither a sink nor in that cone) does not.
#[tokio::test(flavor = "multi_thread")]
async fn run_sinks_node_runs_all_sinks_on_event() -> TestResult {
    let source_calls = Calls::default();

    let mut g = TestGraph::new();
    g.add("emit", emitter(&Calls::default()));
    g.add("source", emitter(&source_calls));
    g.add("sink", |n| n.records());
    g.add_special("trigger", SpecialNode::RunSinks);
    // The sink's cone (source → sink) is wholly independent of emit.
    g.wire("source", 0, "sink", 0);
    g.subscribe("emit", 0, "trigger");

    let mut e = TestEngine::over(g);
    let run = e.run_events([e.event("emit", 0)]).await;

    // The sink cone ran; emit is neither a sink nor in that cone, so it did
    // not. The `RunSinks` node is itself a sink, so it runs its no-op lambda
    // alongside the promoted sinks rather than seeding a cone of its own.
    // The stack pops the later-declared root first, so the portless
    // trigger settles before the cone it promoted.
    assert_eq!(run.ran(), ["trigger", "source", "sink"]);
    assert_eq!(source_calls.count(), 1);
    assert_eq!(run.logs(), ["1"]);
    assert_eq!(run.triggered_events.len(), 1);
    Ok(())
}

/// Without the `RunSinks` sink, firing `emit`'s tick reaches no subscriber,
/// so the same sink cone is left untouched — isolating the sink as the cause.
#[tokio::test(flavor = "multi_thread")]
async fn event_without_run_sinks_sink_runs_nothing() -> TestResult {
    let source_calls = Calls::default();

    let mut g = TestGraph::new();
    g.add("emit", emitter(&Calls::default()));
    g.add("source", emitter(&source_calls));
    g.add("sink", |n| n.sink().input(DataType::Int));
    g.wire("source", 0, "sink", 0);

    let mut e = TestEngine::over(g);
    let run = e.run_events([e.event("emit", 0)]).await;

    assert!(run.ran().is_empty());
    assert_eq!(source_calls.count(), 0);
    Ok(())
}
