use super::*;

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

/// A pure source whose value a test can move under it — the point being
/// that a *pure* node's digest does not notice, so the cached value stands
/// until something re-keys it.
fn shifting_source(cell: Arc<AtomicI64>) -> impl FnOnce(NodeSpec) -> NodeSpec {
    move |n: NodeSpec| {
        n.pure()
            .cache(CacheMode::Ram)
            .output(DataType::Int)
            .compute(move |_| cell.load(Ordering::Relaxed).into())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn simple_compute() -> TestResult {
    let b = Arc::new(AtomicI64::new(5));
    let mut g = TestGraph::new();
    g.add("a", shifting_source(Arc::new(AtomicI64::new(2))));
    g.add("b", shifting_source(b.clone()));
    g.add("sum", |n| {
        n.pure()
            .cache(CacheMode::Ram)
            .input(DataType::Int)
            .input(DataType::Int)
            .output(DataType::Int)
            .compute(|i| (i[0].as_i64().unwrap() + i[1].as_i64().unwrap()).into())
    });
    g.add("mult", |n| {
        n.pure()
            .cache(CacheMode::Ram)
            .input(DataType::Int)
            .input(DataType::Int)
            .output(DataType::Int)
            .compute(|i| (i[0].as_i64().unwrap() * i[1].as_i64().unwrap()).into())
    });
    g.add("print", |n| n.records());
    g.wire("a", 0, "sum", 0);
    g.wire("b", 0, "sum", 1);
    g.wire("sum", 0, "mult", 0);
    g.wire("b", 0, "mult", 1);
    g.wire("mult", 0, "print", 0);

    let mut e = TestEngine::over(g);
    let run = e.run_sinks().await;
    assert_eq!(run.logs(), ["35"], "sum = 2 + 5 = 7, mult = 7 * 5 = 35");

    // Moving external state does not recompute: `b` is pure, so its digest
    // is stable and the cached value stands.
    b.store(7, Ordering::Relaxed);
    let run = e.run_sinks().await;
    assert_eq!(run.logs(), ["35"], "a pure node does not re-read the world");

    // Declaring `b` impure re-keys it: now it re-reads on every run.
    e.edit(|g| g.edit_func("b", |func| func.behavior = FuncBehavior::Impure));
    let run = e.run_sinks().await;
    assert_eq!(run.logs(), ["63"], "sum = 2 + 7 = 9, mult = 9 * 7 = 63");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn required_none_binding_is_stable() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample());
    // sum's first input unbound (required) — sum and downstream can't run.
    e.edit(|g| g.unbind("sum", 0));

    let first = e.run_sinks().await;
    let second = e.run_sinks().await;

    // The schedule is deterministic across runs. What actually *runs* can
    // differ as pure nodes start reusing their cache, but the order cannot
    // flap.
    assert_eq!(first.missing_inputs(), second.missing_inputs());
    assert_eq!(first.missing_inputs(), ["Print", "mult", "sum"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn schedule_stable_across_repeated_runs() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample());

    // Three runs held at once — an outcome is a snapshot, so they can be
    // compared rather than re-derived.
    let run1 = e.run_sinks().await;
    let run2 = e.run_sinks().await;
    let run3 = e.run_sinks().await;

    // The first run executes everything; once the pure upstream is cached,
    // runs 2 and 3 must schedule identically — this guards the reused
    // per-run buffers being reset cleanly, since a missed reset would drift.
    assert_eq!(run1.ran(), ["get_b", "get_a", "sum", "mult", "Print"]);
    assert_eq!(run2.ran(), ["Print"]);
    assert_eq!(run2.ran(), run3.ran());
    assert_eq!(
        run2.cached(),
        ["get_a", "get_b", "mult", "sum"],
        "everything the second run did not re-run was served from cache"
    );

    // The cached product stays correct every run: sum(1 + 11 = 12) * get_b(11) = 132.
    assert_eq!(e.output_i64("mult", 0), Some(132));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cached_upstream_output_reused_after_rebinding() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample());
    e.run_sinks().await;

    // Switch mult to const inputs: its upstream leaves the run.
    e.edit(|g| {
        g.constant("mult", 0, 2i64);
        g.constant("mult", 1, 21i64);
    });
    assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);

    // Switch one back to a bind from the *cached* get_b — mult re-executes,
    // but its producer is served from cache rather than re-run.
    e.edit(|g| g.wire("get_b", 0, "mult", 0));
    assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);
    Ok(())
}

/// Output buffers are wiped before a re-running node is invoked, so an
/// unwritten output cannot retain a prior run's value. This sink has no
/// demanded outputs, so leaving one port `Unbound` is valid.
#[tokio::test(flavor = "multi_thread")]
async fn unwritten_output_port_is_cleared_before_reexecution() -> TestResult {
    use crate::async_lambda;

    let invocations = Arc::new(AtomicUsize::new(0));
    let mut g = TestGraph::new();
    g.add("partial_writer", |n| {
        let invocations = invocations.clone();
        n.pure()
            .sink()
            // Const-bound below; changing that const is what re-keys the
            // digest between runs.
            .optional(DataType::Int)
            .output(DataType::Int)
            .output(DataType::Int)
            // Retained so the in-place buffer reuse is observable at all.
            .cache(CacheMode::Ram)
            .lambda(async_lambda!(
                move |Invocation { outputs, .. }| { invocations = invocations.clone() } => {
                    let run = invocations.fetch_add(1, Ordering::Relaxed);
                    outputs[0] = StaticValue::Int(100 + run as i64).into();
                    if run == 0 {
                        // Only the first run writes the second port.
                        outputs[1] = StaticValue::Int(20).into();
                    }
                    Ok(())
                }
            ))
    });
    g.constant("partial_writer", 0, 0i64);

    let mut e = TestEngine::over(g);

    let run = e.run_sinks().await;
    assert!(run.errored().is_empty());
    assert_eq!(e.output_i64("partial_writer", 0), Some(100));
    assert_eq!(
        e.output_i64("partial_writer", 1),
        Some(20),
        "run 1 writes both"
    );

    // Invalidate the pure node while keeping its resident buffer available
    // for the next invocation. Run 2 writes only port 0, so port 1 must not
    // retain run 1's value.
    e.edit(|g| g.constant("partial_writer", 0, 1i64));
    let run = e.run_sinks().await;

    assert!(run.errored().is_empty());
    assert_eq!(
        e.output_i64("partial_writer", 0),
        Some(101),
        "port 0 rewritten"
    );
    assert!(
        matches!(e.output("partial_writer", 1), Some(DynamicValue::Unbound)),
        "the unwritten port is cleared before invoke, not left holding 20"
    );
    Ok(())
}
