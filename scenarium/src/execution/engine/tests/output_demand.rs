use super::*;

use crate::async_lambda;
use crate::graph::func::lambda::OutputDemand;

#[tokio::test(flavor = "multi_thread")]
async fn unused_output_marked_skip() {
    let seen: Arc<Mutex<Vec<OutputDemand>>> = Arc::new(Mutex::new(Vec::new()));

    let mut g = TestGraph::new();
    g.add("split", |n| {
        let seen = seen.clone();
        n.output(DataType::Int)
            .output(DataType::Int)
            .lambda(async_lambda!(
                move |Invocation { demand, outputs, .. }| { seen = seen.clone() } => {
                    seen.lock().await.extend_from_slice(demand);
                    outputs[0] = ConstValue::Int(1).into();
                    outputs[1] = ConstValue::Int(2).into();
                    Ok(())
                }
            ))
    });
    g.add("sink", |n| n.sink().input(DataType::Int));
    // Consume only output 0; output 1 has no consumer.
    g.wire("split", 0, "sink", 0);

    let mut e = TestEngine::over(g);
    e.run_sinks().await;

    assert_eq!(
        e.demand("split"),
        [OutputDemand::Produce, OutputDemand::Skip]
    );
    assert_eq!(e.readers("split"), [1, 0]);
    assert_eq!(
        *seen.lock().await,
        [OutputDemand::Produce, OutputDemand::Skip],
        "the lambda saw the same demand the sweep resolved"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cached_node_reruns_when_a_previously_skipped_output_becomes_needed() {
    let calls = Arc::new(Mutex::new(0));
    let received = Arc::new(Mutex::new(Vec::new()));

    let mut g = TestGraph::new();
    g.add("split", |n| {
        let calls = calls.clone();
        n.pure()
            .cache(CacheMode::Ram)
            .output(DataType::Int)
            .output(DataType::Int)
            .lambda(async_lambda!(
                move |Invocation { demand, outputs, .. }| { calls = calls.clone() } => {
                    *calls.lock().await += 1;
                    if !demand[0].is_skip() {
                        outputs[0] = ConstValue::Int(10).into();
                    }
                    if !demand[1].is_skip() {
                        outputs[1] = ConstValue::Int(20).into();
                    }
                    Ok(())
                }
            ))
    });
    let sink = |received: Arc<Mutex<Vec<i64>>>| {
        move |n: NodeSpec| {
            n.sink().input(DataType::Int).lambda(async_lambda!(
                move |Invocation { inputs, .. }| { received = received.clone() } => {
                    received.lock().await.push(inputs[0].as_i64().unwrap());
                    Ok(())
                }
            ))
        }
    };
    g.add("sink_a", sink(received.clone()));
    g.wire("split", 0, "sink_a", 0);

    let mut e = TestEngine::over(g);
    e.run_sinks().await;

    // A second consumer arrives on the output the first run skipped: the
    // cached value does not cover the new demand, so `split` must re-run.
    e.edit(|g| {
        g.add("sink_b", sink(received.clone()));
        g.wire("split", 1, "sink_b", 0);
    });
    e.run_sinks().await;

    assert_eq!(*calls.lock().await, 2);
    let mut received = received.lock().await.clone();
    received.sort_unstable();
    assert_eq!(received, [10, 10, 20]);
}
