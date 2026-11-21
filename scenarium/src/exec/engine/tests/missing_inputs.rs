use super::*;

/// Which nodes a plan blocks for want of a required input, and which it clears, as one edit of
/// the sample (`get_a`, `get_b` → `sum`; `sum`, `get_b` → `mult` → `Print`) changes them:
///
/// - `sum[0]` unbound blocks `sum`, and the verdict travels down `mult` and `Print`; `get_a`,
///   read by nothing left, is never reached, and `get_b` still stands.
/// - The same with `mult[0]` optional: a binding to a blocked producer blocks even an optional
///   input, as optionality excuses only an unbound one.
/// - `mult[0]` unbound and optional: a deliberate no-value blocks nothing, and `sum` and `get_a`,
///   read by nothing left, are never reached.
#[tokio::test]
async fn a_missing_required_input_blocks_its_node_and_every_consumer() {
    struct Case {
        edit: fn(&mut TestGraph),
        missing: &'static [&'static str],
        runnable: &'static [&'static str],
    }
    let cases = [
        Case {
            edit: |g| g.unbind("sum", 0),
            missing: &["Print", "mult", "sum"],
            runnable: &["get_b"],
        },
        Case {
            edit: |g| {
                g.unbind("sum", 0);
                g.edit_func("mult", |func| func.ports.inputs[0].required = false);
            },
            missing: &["Print", "mult", "sum"],
            runnable: &["get_b"],
        },
        Case {
            edit: |g| {
                g.unbind("mult", 0);
                g.edit_func("mult", |func| func.ports.inputs[0].required = false);
            },
            missing: &[],
            runnable: &["Print", "get_b", "mult"],
        },
    ];
    for (idx, case) in cases.iter().enumerate() {
        let mut e = TestEngine::over(TestGraph::sample());
        e.edit(case.edit);
        let plan = e.plan_sinks().await;
        assert_eq!(plan.missing_inputs(), case.missing, "case {idx}");
        assert_eq!(plan.runnable(), case.runnable, "case {idx}");
    }
}

/// A run names the exact port that failed rather than the node as a whole, and the verdict is
/// stable: what *runs* can differ as pure nodes start reusing their cache, but the missing set
/// cannot flap.
#[tokio::test]
async fn a_run_names_the_missing_port_and_the_verdict_holds_across_runs() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| g.unbind("sum", 0));

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

/// Executing counterpart: an optional bind to a gated upstream gates the consumer chain, so the
/// executor never reads the absent output (else the worker panics in `collect_inputs` with
/// "missing output values"). The planned-only siblings above never execute, so can't catch it.
#[tokio::test]
async fn optional_bind_to_gated_upstream_is_gated() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| {
        // sum's required input[0] unbound → sum missing-required → gated.
        g.unbind("sum", 0);
        // mult[0] (required) gets a real value; mult[1] is the only bind to
        // the gated sum and is *optional*, so this exercises optional-bind
        // propagation specifically. mult and print end up gated.
        g.wire("get_b", "mult");
        g.wire("sum", ("mult", 1));
        g.edit_func("mult", |func| func.ports.inputs[1].required = false);
    });

    // The chain is gated and nothing runs.
    let run = e.run_sinks().await;

    assert_eq!(run.missing_inputs(), ["Print", "mult", "sum"]);
    assert_eq!(
        run.ran(),
        Vec::<&str>::new(),
        "the gated chain never runs, so it never reads sum's absent output — \
         and `get_b`, whose only consumer is gated, is cut with it"
    );
}
