use super::*;

/// A required input left unbound blocks its node, and the verdict travels
/// down every consumer — so the whole tail of the chain is out of the run
/// while its sources still stand. The report names the exact port that
/// failed rather than the node as a whole, and the verdict is stable: what
/// actually *runs* can differ as pure nodes start reusing their cache, but
/// the missing set cannot flap.
#[tokio::test(flavor = "multi_thread")]
async fn required_missing_propagates_downstream() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| g.unbind("sum", 0));

    let plan = e.plan_sinks().await;

    assert_eq!(plan.missing_inputs(), ["Print", "mult", "sum"]);
    assert_eq!(
        plan.runnable(),
        ["get_b"],
        "the one source still feeding something stands; unbinding sum[0] left \
         `get_a` reading into nothing, so the backward walk never reaches it"
    );

    let first = e.run_sinks().await;
    let second = e.run_sinks().await;

    assert_eq!(
        first.missing_ports("sum"),
        [0],
        "port 1 is still bound, so the run names the port that failed"
    );
    assert_eq!(first.missing_inputs(), ["Print", "mult", "sum"]);
    assert_eq!(second.missing_inputs(), first.missing_inputs());
}

/// A *binding* to a missing-required producer propagates even through an
/// **optional** input: the wired value can't be delivered, so the consumer
/// (and its consumers) are missing too. Optionality only excuses an
/// *unbound* input (see `optional_unbound_does_not_propagate`), not a
/// binding to a broken upstream.
#[tokio::test]
async fn optional_bind_to_missing_propagates() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| {
        // sum missing-required; mult[0] stays bound to sum but goes optional.
        g.unbind("sum", 0);
        g.edit_func("mult", |func| func.inputs[0].required = false);
    });

    let plan = e.plan_sinks().await;

    assert_eq!(plan.missing_inputs(), ["Print", "mult", "sum"]);
    assert_eq!(plan.runnable(), ["get_b"]);
}

/// The contrast to `optional_bind_to_missing_propagates`: an optional input
/// left **unbound** is a deliberate no-value, so it does not flag the node
/// missing — it runs with its default.
#[tokio::test]
async fn optional_unbound_does_not_propagate() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| {
        g.unbind("mult", 0);
        g.edit_func("mult", |func| func.inputs[0].required = false);
    });

    let plan = e.plan_sinks().await;

    assert!(plan.missing_inputs().is_empty());
    assert!(plan.runnable().contains(&"mult"));
}

/// Executing counterpart: an optional bind to a gated upstream gates the
/// consumer chain, so the executor never reads the absent output. Regression
/// for the worker panicking in `collect_inputs` ("missing output values") —
/// the planned-only siblings above can't catch it since they never execute.
#[tokio::test(flavor = "multi_thread")]
async fn optional_bind_to_gated_upstream_is_gated() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| {
        // sum's required input[0] unbound → sum missing-required → gated.
        g.unbind("sum", 0);
        // mult[0] (required) gets a real value; mult[1] is the only bind to
        // the gated sum and is *optional*, so this exercises optional-bind
        // propagation specifically. mult and print end up gated.
        g.wire("get_b", 0, "mult", 0);
        g.wire("sum", 0, "mult", 1);
        g.edit_func("mult", |func| func.inputs[1].required = false);
    });

    // Pre-fix, this panicked the worker; now the chain is gated and nothing runs.
    let run = e.run_sinks().await;

    assert_eq!(run.missing_inputs(), ["Print", "mult", "sum"]);
    assert_eq!(
        run.ran(),
        [] as [&str; 0],
        "the gated chain never runs, so it never reads sum's absent output — \
         and `get_b`, whose only consumer is gated, is cut with it"
    );
}
