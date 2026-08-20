use super::*;

use common::FloatExt;

#[tokio::test(flavor = "multi_thread")]
async fn removing_node_rebuilds_id_keyed_edges() {
    let mut e = TestEngine::over(TestGraph::sample_values(2, 5));
    assert_eq!(e.engine.compiled().e_nodes.len(), 5);

    // Remove get_b — a middle node feeding sum[1] and mult[1], both optional.
    // The surviving id-keyed bindings must remain valid across the remap.
    e.edit(|g| {
        g.remove("get_b");
    });
    assert_eq!(e.engine.compiled().e_nodes.len(), 4);

    e.run_sinks().await;

    // sum = get_a(2) + none(0) = 2; mult = sum(2) * none(default 1) = 2.
    assert!(matches!(
        e.inputs("sum")[0],
        Some(DynamicValue::Static(ConstValue::Float(v))) if v.approximately_eq(2.0)
    ));
    assert!(e.inputs("sum")[1].is_none());
    assert_eq!(e.output_i64("sum", 0), Some(2));
    assert_eq!(e.output_i64("mult", 0), Some(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_graph_executes_cleanly() {
    let mut e = TestEngine::over(TestGraph::new());
    assert!(e.engine.is_empty());

    let run = e.run_sinks().await;

    assert_eq!(run.ran_node_count, 0);
    assert!(run.ran().is_empty());
    assert!(run.errored().is_empty());
    assert!(run.missing_inputs().is_empty());
}

/// Two independent chains (`a → print_a`, `b → print_b`) both execute, and
/// both sources are Pure, so their outputs are cached across runs. Removing
/// one chain must preserve the survivor's id-keyed slot.
#[tokio::test(flavor = "multi_thread")]
async fn cached_output_survives_node_removal() {
    let (calls_a, calls_b) = (Calls::default(), Calls::default());
    let mut g = TestGraph::new();
    g.add("a", |n| n.counted(2i64, &calls_a).cache(CacheMode::Ram));
    g.add("b", |n| n.counted(5i64, &calls_b).cache(CacheMode::Ram));
    g.add("print_a", |n| n.records());
    g.instance("print_b", "print_a");
    g.wire("a", 0, "print_a", 0);
    g.wire("b", 0, "print_b", 0);

    let mut e = TestEngine::over(g);
    let run = e.run_sinks().await;
    assert_eq!(run.ran_node_count, 4, "both sinks and both sources");
    let mut logged = run.logs();
    logged.sort_unstable();
    assert_eq!(logged, ["2", "5"]);
    assert_eq!(calls_a.count(), 1);
    assert_eq!(calls_b.count(), 1);

    e.edit(|g| {
        g.remove("b");
        g.remove("print_b");
    });
    let run = e.run_sinks().await;

    assert_eq!(
        calls_a.count(),
        1,
        "the survivor must not recompute after an unrelated node's removal"
    );
    assert!(run.cached().contains(&"a"));
    assert_eq!(e.output_i64("a", 0), Some(2));
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_structural_churn_stays_correct() {
    // Grow→shrink the graph repeatedly on ONE engine, re-executing each
    // step. Stresses the packed pools and the id-keyed rebuild across many
    // updates (pools grow 2→4 then shrink 4→2 each round).
    let mut g = TestGraph::new();
    g.add("a", |n| n.returns(2i64));
    g.add("print_a", |n| n.records());
    g.wire("a", 0, "print_a", 0);

    let mut e = TestEngine::over(g);
    e.run_sinks().await;

    for round in 0..3 {
        e.edit(|g| {
            g.add("b", |n| n.returns(5i64));
            g.instance("print_b", "print_a");
            g.wire("b", 0, "print_b", 0);
        });
        assert_eq!(e.engine.compiled().e_nodes.len(), 4, "round {round} grow");
        let run = e.run_sinks().await;
        let mut logged = run.logs();
        logged.sort_unstable();
        assert_eq!(logged, ["2", "5"], "round {round} grow values");

        e.edit(|g| {
            g.remove("b");
            g.remove("print_b");
        });
        assert_eq!(e.engine.compiled().e_nodes.len(), 2, "round {round} shrink");
        let run = e.run_sinks().await;
        assert_eq!(run.logs(), ["2"], "round {round} shrink values");
    }
}

#[tokio::test]
async fn planning_a_cycle_names_the_node_that_closes_it() {
    let mut e = TestEngine::over(TestGraph::sample());
    // Close the loop: sum[0] ← mult, and mult already depends on sum.
    e.edit(|g| g.wire("mult", 0, "sum", 0));

    let error = e
        .try_plan(RunSeeds::sinks())
        .await
        .expect_err("a cyclic graph cannot be planned");

    assert!(
        matches!(error, Error::CycleDetected { node_id } if node_id == e.id("mult")),
        "unexpected error: {error:?}"
    );
}
