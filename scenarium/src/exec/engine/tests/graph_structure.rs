use super::*;

#[tokio::test]
async fn basic_run() {
    let mut e = TestEngine::over(TestGraph::sample());

    let plan = e.plan_sinks().await;

    assert_eq!(plan.scheduled(), ["get_b", "get_a", "sum", "mult", "Print"]);
    assert_eq!(
        plan.runnable(),
        ["Print", "get_a", "get_b", "mult", "sum"],
        "an unedited fixture blocks nothing"
    );
    assert!(plan.missing_inputs().is_empty());

    // get_a→sum[0], get_b→sum[1]+mult[1], sum→mult[0], mult→print[0].
    for name in ["get_a", "get_b", "sum", "mult"] {
        assert_eq!(e.demand(name), [OutputDemand::Produce], "{name} demand");
    }
    assert_eq!(e.readers("get_a"), [1]);
    assert_eq!(e.readers("get_b"), [2], "feeds both sum[1] and mult[1]");
    assert_eq!(e.readers("sum"), [1]);
    assert_eq!(e.readers("mult"), [1]);

    assert!(e.engine.compiled().by_id(e.id("Print")).sink);
}

#[tokio::test]
async fn updates_after_graph_change() {
    let mut e = TestEngine::over(TestGraph::sample());
    // Rewire mult to the sources directly, bypassing sum.
    e.edit(|g| {
        g.wire("get_a", "mult");
        g.wire("get_b", ("mult", 1));
    });

    let plan = e.plan_sinks().await;

    assert_eq!(
        plan.scheduled(),
        ["get_b", "get_a", "mult", "Print"],
        "sum is no longer in any sink's cone"
    );
    for name in ["get_a", "get_b", "mult"] {
        assert_eq!(e.demand(name), [OutputDemand::Produce], "{name} demand");
        assert_eq!(e.readers(name), [1], "{name} now has exactly one consumer");
    }
    assert!(e.demand("Print").is_empty());
}
