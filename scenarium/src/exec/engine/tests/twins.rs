//! Twins: a node whose digest an earlier node of the run shares takes that node's values.

use std::sync::atomic::{AtomicI64, Ordering};

use super::*;

/// `src` → `double` and its twin `double_2`, a second node of the same func on the same input,
/// each into a sink that adds what it reads to a total. The run invokes the func once: the twin
/// takes its 6, so the sinks read 6 each (total 12). `double_2` leads, as the planner's stack
/// walks the roots from the last placed, `sink_d`, down, and so reaches `sink_b` before
/// `sink_a`: it reports executed and `double` cached. An impure node has no digest, so its twin
/// still runs.
#[tokio::test]
async fn a_twin_takes_its_leaders_values_and_runs_nothing() {
    let calls = Calls::default();
    let noisy = Calls::default();
    let total = Arc::new(AtomicI64::new(0));
    let mut g = TestGraph::new();
    g.add("src", |n| {
        n.pure().output(DataType::Int).compute(|_| 3.into())
    });
    g.add("double", |n| {
        n.pure()
            .input(DataType::Int)
            .output(DataType::Int)
            .compute(calls.counting(|inputs| (inputs[0].as_i64().unwrap() * 2).into()))
    });
    g.instance("double_2", "double");
    g.add("noisy", |n| {
        n.input(DataType::Int)
            .output(DataType::Int)
            .compute(noisy.returning(1))
    });
    g.instance("noisy_2", "noisy");
    for (from, sink) in [
        ("double", "sink_a"),
        ("double_2", "sink_b"),
        ("noisy", "sink_c"),
        ("noisy_2", "sink_d"),
    ] {
        let total = Arc::clone(&total);
        g.add(sink, |n| {
            n.sink().input(DataType::Int).observes(move |inputs| {
                total.fetch_add(inputs[0].as_i64().unwrap(), Ordering::Relaxed);
            })
        });
        g.wire("src", from);
        g.wire(from, sink);
    }
    let mut e = TestEngine::over(g);
    let run = e.run_sinks().await;
    assert_eq!(calls.count(), 1, "the twin runs nothing");
    assert_eq!(noisy.count(), 2, "an impure node has no digest to share");
    assert_eq!(total.load(Ordering::Relaxed), 6 + 6 + 1 + 1);
    assert_eq!(
        run.ran(),
        [
            "src", "noisy_2", "sink_d", "noisy", "sink_c", "double_2", "sink_b", "sink_a"
        ]
    );
    assert_eq!(run.cached(), ["double"]);
}

/// A leader that fails leaves its twin skipped as errored upstream, so neither sink reads a
/// value; the func ran once. `double_2` leads, as above: it fails with the lambda's error, and
/// `double` is skipped.
#[tokio::test]
async fn a_failed_leader_skips_its_twin() {
    let mut g = TestGraph::new();
    g.add("src", |n| {
        n.pure().output(DataType::Int).compute(|_| 3.into())
    });
    g.add("double", |n| {
        n.pure().input(DataType::Int).output(DataType::Int)
    });
    g.instance("double_2", "double");
    for (from, sink) in [("double", "sink_a"), ("double_2", "sink_b")] {
        g.add(sink, |n| n.sink().input(DataType::Int));
        g.wire("src", from);
        g.wire(from, sink);
    }
    g.fails("double", "boom");
    let mut e = TestEngine::over(g);
    let run = e.run_sinks().await;
    assert!(
        matches!(run.error("double_2"), Some(RunError::Invoke(error)) if error.to_string() == "boom"),
        "{:?}",
        run.error("double_2")
    );
    assert!(matches!(
        run.error("double"),
        Some(RunError::SkippedUpstream)
    ));
    assert_eq!(run.ran(), ["src"]);
}
