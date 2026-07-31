use super::*;

#[tokio::test]
async fn basic_run() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample());

    let plan = e.plan_sinks().await?;

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
    Ok(())
}

#[tokio::test]
async fn updates_after_graph_change() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample());
    // Rewire mult to the sources directly, bypassing sum.
    e.edit(|g| {
        g.wire("get_a", 0, "mult", 0);
        g.wire("get_b", 0, "mult", 1);
    });

    let plan = e.plan_sinks().await?;

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
    Ok(())
}

#[test]
fn update_rejects_func_missing_from_lib_and_keeps_prior_program() {
    let mut e = TestEngine::over(TestGraph::sample());
    assert_eq!(e.engine.compiled().e_nodes.len(), 5);

    // Recompiling the same graph against a library that defines none of its
    // funcs is rejected with a message naming a missing func.
    e.graph.library = Library::default();
    let CompileError { message } = e.try_reinstall().unwrap_err();
    assert!(
        message.contains("absent from the library"),
        "message should explain the missing func, got: {message}"
    );

    // The rejection happens before any mutation, so the prior program is
    // left intact rather than torn down.
    assert_eq!(e.engine.compiled().e_nodes.len(), 5);
}
