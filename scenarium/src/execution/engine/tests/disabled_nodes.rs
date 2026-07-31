use super::*;

use crate::execution::schedule::NodeState;

/// Disabling `sum` retains it in the compiled program but excludes it from
/// the plan. Its consumer `mult` sees the disabled producer as unavailable,
/// so the missing-required-input flag propagates downstream.
#[tokio::test]
async fn disabled_node_stays_compiled_but_breaks_downstream() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| g.disable("sum"));

    let plan = e.plan_sinks().await;

    assert!(
        e.engine.compiled().by_id(e.id("sum")).disabled,
        "the compiled node retains its authored disabled state"
    );
    assert_eq!(
        plan.state("sum"),
        NodeState::Disabled,
        "an unseeded disabled node stays structural but outside execution order"
    );
    assert_eq!(
        plan.missing_inputs(),
        ["Print", "mult"],
        "the consumers lost their transitive producer"
    );
}

/// With `mult`'s sum-fed input made optional, disabling `sum` no longer
/// breaks the chain: `sum` is skipped but `get_b → mult → print` still
/// runs (mirrors `optional_unbound_does_not_propagate`, but via the
/// disable flag rather than a cleared binding).
#[tokio::test]
async fn disabled_upstream_with_optional_consumer_still_runs() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| {
        g.disable("sum");
        g.edit_func("mult", |func| func.inputs[0].required = false);
    });

    let plan = e.plan_sinks().await;

    assert_eq!(plan.scheduled(), ["get_b", "mult", "Print"]);
}

/// …and the same shape must survive **execution**, not just planning.
///
/// The consumer is schedulable and the disabled producer is not in
/// `process_order` — but resolution marked every bound producer live
/// regardless, so it registered a reader for an output nothing would
/// ever write. Collecting the consumer's inputs then demanded that
/// output: on a cold cache the run died on `a resolved producer
/// output must be resident when consumed`, and on a warm one it
/// silently served whatever the producer had left in RAM from before
/// it was disabled, as if it were this run's value.
///
/// `mult`'s second port is the optional one, so the disabled producer feeds
/// *that*; unbound is what optional means, and `arith`'s identity of 1 is
/// what reads it.
#[tokio::test]
async fn a_disabled_producer_on_an_optional_input_delivers_unbound() {
    let mut g = TestGraph::new();
    g.add("src", |n| n.returns(7i64));
    g.add("disabled", |n| {
        n.pure()
            .input(DataType::Int)
            .output(DataType::Int)
            .compute(|inputs| inputs[0].as_i64().unwrap_or_default().into())
    });
    g.add("mult", |n| n.mult());
    g.add("print", |n| n.records());
    g.wire("src", 0, "disabled", 0);
    g.wire("src", 0, "mult", 0);
    g.wire("disabled", 0, "mult", 1);
    g.wire("mult", 0, "print", 0);

    let mut e = TestEngine::over(g);
    e.edit(|g| g.disable("disabled"));

    let run = e.run_sinks().await;

    assert_eq!(
        run.ran(),
        ["src", "mult", "print"],
        "the disabled producer stays out of the run",
    );
    assert_eq!(
        run.logs(),
        ["7"],
        "the optional input read as unbound, so `mult` multiplied by its \
         own identity of 1 rather than reading a value nothing wrote",
    );
}
