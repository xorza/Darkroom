//! Twins: a node whose digest an earlier node of the run shares takes that node's values.

use std::sync::atomic::{AtomicI64, Ordering};

use super::*;

/// `src` → `double` and its twin `double_2`, a second node of the same func on the same input,
/// each into a sink that adds what it reads to a total. The run invokes the func once: the twin
/// takes its 6, so the sinks read 6 each (total 12), and one of them reports executed, the
/// other cached. An impure node has no digest, so its twin still runs.
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
    // The first of the two in the run's order leads, the other takes its values.
    let doubles = ["double", "double_2"];
    let ran: Vec<&str> = run
        .ran()
        .into_iter()
        .filter(|name| doubles.contains(name))
        .collect();
    let cached: Vec<&str> = run
        .cached()
        .into_iter()
        .filter(|name| doubles.contains(name))
        .collect();
    assert_eq!((ran.len(), cached.len()), (1, 1), "{ran:?} {cached:?}");
    assert_ne!(ran, cached);
}

/// A leader that fails leaves its twin skipped as errored upstream, so neither sink reads a
/// value; the func ran once.
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
    let errors: Vec<_> = ["double", "double_2"]
        .map(|name| run.error(name).cloned())
        .into_iter()
        .collect();
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, Some(RunError::Invoke(_))))
            && errors
                .iter()
                .any(|error| matches!(error, Some(RunError::SkippedUpstream))),
        "{errors:?}"
    );
    assert!(
        run.ran().is_empty() || run.ran() == ["src"],
        "{:?}",
        run.ran()
    );
}
