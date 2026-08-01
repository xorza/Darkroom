use super::*;

use crate::execution::report::NodeExecutionStatus;

/// Library drift: wiring that references ports/events the library no
/// longer declares must still compile — the dangling binding degrades
/// to unbound (a required input reports missing), a dangling
/// subscription and pin wire nothing.
#[tokio::test(flavor = "multi_thread")]
async fn dangling_wiring_compiles_and_reports_missing_input() {
    let mut e = TestEngine::over(TestGraph::sample());
    // sum's required input 0 bound to an output `get_a` doesn't have, plus a
    // subscription to an event it doesn't emit — the drift a changed library
    // leaves behind. Neither may fail the compile.
    e.edit(|g| {
        g.wire("get_a", 9, "sum", 0);
        g.subscribe("get_a", 9, "sum");
    });

    let run = e.run_sinks().await;

    assert_eq!(
        run.missing_ports("sum"),
        [0],
        "the dangling binding degrades to a missing input on that exact port"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn executed_nodes_reported() {
    let mut e = TestEngine::over(TestGraph::sample());

    let run = e.run_sinks().await;

    assert_eq!(run.ran(), ["get_b", "get_a", "sum", "mult", "Print"]);
    assert_eq!(run.ran_node_count, 5);
    assert!(run.errored().is_empty());
    assert!(run.missing_inputs().is_empty());

    for name in run.ran() {
        let Some(NodeExecutionStatus::Executed { elapsed_secs }) = run.status(name) else {
            panic!("{name} ran, so it reports an elapsed time");
        };
        assert!(*elapsed_secs >= 0.0, "{name} has negative elapsed_secs");
    }
}
