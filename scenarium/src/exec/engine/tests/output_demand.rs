use common::MutexExt;
use std::sync::Mutex;

use super::*;

use crate::async_lambda;

#[tokio::test]
async fn unused_output_marked_skip() {
    let seen: Arc<Mutex<Vec<OutputDemand>>> = Arc::new(Mutex::new(Vec::new()));

    let mut g = TestGraph::new();
    g.add("split", |n| {
        let seen = Arc::clone(&seen);
        n.output(DataType::Int)
            .output(DataType::Int)
            .lambda(async_lambda!(
                move |Invocation { demand, outputs, .. }| { seen = Arc::clone(&seen) } => {
                    seen.lock_unpoisoned().extend_from_slice(demand);
                    outputs[0] = ConstValue::Int(1).into();
                    outputs[1] = ConstValue::Int(2).into();
                    Ok(())
                }
            ))
    });
    g.add("sink", |n| n.sink().input(DataType::Int));
    // Consume only output 0; output 1 has no consumer.
    g.wire("split", "sink");

    let mut e = TestEngine::over(g);
    e.run_sinks().await;

    assert_eq!(
        e.demand("split"),
        [OutputDemand::Produce, OutputDemand::Skip]
    );
    assert_eq!(e.readers("split"), [1, 0]);
    assert_eq!(
        *seen.lock_unpoisoned(),
        [OutputDemand::Produce, OutputDemand::Skip],
        "the lambda saw the same demand the sweep resolved"
    );
}

/// A pure, kept node of two outputs, 10 and 20, that writes only what is demanded, counting its
/// runs in `calls`.
fn split(calls: &Calls) -> impl FnOnce(NodeSpec) -> NodeSpec {
    let calls = calls.clone();
    move |n| {
        n.pure()
            .cache(CacheMode::ALWAYS)
            .output(DataType::Int)
            .output(DataType::Int)
            .lambda(async_lambda!(
                move |Invocation { demand, outputs, .. }| { calls = calls.clone() } => {
                    calls.bump();
                    if !demand[0].is_skip() {
                        outputs[0] = ConstValue::Int(10).into();
                    }
                    if !demand[1].is_skip() {
                        outputs[1] = ConstValue::Int(20).into();
                    }
                    Ok(())
                }
            ))
    }
}

/// A sink that records the integer it reads into `received`.
fn recording_sink(received: &Arc<Mutex<Vec<i64>>>) -> impl FnOnce(NodeSpec) -> NodeSpec {
    let received = Arc::clone(received);
    move |n| {
        n.sink().input(DataType::Int).lambda(async_lambda!(
            move |Invocation { inputs, .. }| { received = Arc::clone(&received) } => {
                received.lock_unpoisoned().push(inputs[0].as_i64().unwrap());
                Ok(())
            }
        ))
    }
}

#[tokio::test]
async fn cached_node_reruns_when_a_previously_skipped_output_becomes_needed() {
    let calls = Calls::default();
    let received = Arc::new(Mutex::new(Vec::new()));

    let mut g = TestGraph::new();
    g.add("split", split(&calls));
    g.add("sink_a", recording_sink(&received));
    g.wire("split", "sink_a");

    let mut e = TestEngine::over(g);
    e.run_sinks().await;

    // A second consumer arrives on the output the first run skipped: the
    // cached value does not cover the new demand, so `split` must re-run.
    e.edit(|g| {
        g.add("sink_b", recording_sink(&received));
        g.wire(("split", 1), "sink_b");
    });
    e.run_sinks().await;

    assert_eq!(calls.count(), 2);
    let mut received = received.lock_unpoisoned().clone();
    received.sort_unstable();
    assert_eq!(received, [10, 10, 20]);
}

/// A node re-run for an output it skipped keeps the one it made before under the same key: read
/// on port 0, then port 1, then port 0 again, `split` runs twice, not three times, and holds 10
/// and 20 after its second run.
#[tokio::test]
async fn a_rerun_for_a_skipped_output_keeps_the_one_made_before() {
    let calls = Calls::default();
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut g = TestGraph::new();
    g.add("split", split(&calls));
    g.add("sink_a", recording_sink(&received));
    g.add("sink_b", recording_sink(&received));
    g.wire("split", "sink_a");
    g.wire(("split", 1), "sink_b");
    let mut e = TestEngine::over(g);

    e.run_nodes(["sink_a"]).await;
    e.run_nodes(["sink_b"]).await;
    assert_eq!(e.output_i64("split", 0), Some(10));
    assert_eq!(e.output_i64("split", 1), Some(20));
    let run = e.run_nodes(["sink_a"]).await;

    assert_eq!(calls.count(), 2);
    assert_eq!(run.cached(), ["split"]);
    assert_eq!(*received.lock_unpoisoned(), [10, 20, 10]);
}
