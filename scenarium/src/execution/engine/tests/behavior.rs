use super::*;

use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test(flavor = "multi_thread")]
async fn execute_emits_started_then_finished_progress_per_node() {
    use crate::execution::report::RunPhase;

    let mut e = TestEngine::over(TestGraph::sample());
    let ReportedRun { run, progress } = e.run_sinks_reporting().await;

    // Events come in Started→Finished pairs for the *same* node: the
    // executor is sequential, so each node brackets before the next starts.
    assert_eq!(progress.len() % 2, 0, "paired events");
    let mut started: Vec<&str> = Vec::new();
    for [
        (started_name, started_phase),
        (finished_name, finished_phase),
    ] in progress.as_chunks::<2>().0
    {
        assert!(
            matches!(started_phase, RunPhase::Started { .. }),
            "first of pair is Started",
        );
        assert_eq!(started_name, finished_name, "one node brackets itself");
        assert!(
            matches!(finished_phase, RunPhase::Finished { elapsed_secs } if *elapsed_secs >= 0.0),
            "second of pair is Finished with non-negative elapsed",
        );
        started.push(started_name);
    }

    // The progressed order equals the run's own order, and covers exactly
    // the nodes that finally executed.
    assert_eq!(started, run.ran());
    assert_eq!(started.len(), run.ran_node_count);
}

#[tokio::test(flavor = "multi_thread")]
async fn execute_honors_cancel_flag_and_marks_cancelled() {
    let mut e = TestEngine::over(TestGraph::sample());

    // Pre-tripped: the executor breaks at the first loop-top check, so no
    // node runs and the run is flagged cancelled.
    let tripped = CancelToken::new();
    tripped.cancel();
    let run = e.run_cancellable(RunSeeds::sinks(), tripped).await;
    assert!(run.cancelled, "pre-tripped run is cancelled");
    assert_eq!(run.ran_node_count, 0, "no node runs when cancel is set");

    // A fresh token runs the whole graph — nothing was cached by the run
    // that aborted above.
    let run = e
        .run_cancellable(RunSeeds::sinks(), CancelToken::new())
        .await;
    assert!(!run.cancelled);
    assert_eq!(run.ran_node_count, 5, "all nodes run when not cancelled");
}

/// A node cancelled *mid-invoke* (the run is cancelled while its lambda
/// runs) must not be reported executed and must not cache its partial
/// output — otherwise the next run treats it as already computed. Models
/// "start a run, immediately cancel it": the in-flight node bails with `Ok`
/// but its result is bogus.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_mid_invoke_drops_in_flight_node_and_reruns() {
    use crate::async_lambda;

    // Trips the cancel on its first invoke only, so the re-run completes.
    let cancel_first = Arc::new(AtomicBool::new(true));
    let mut g = TestGraph::new();
    g.add("self_cancel", |n| {
        let cancel_first = Arc::clone(&cancel_first);
        n.pure().sink().output(DataType::Int).lambda(async_lambda!(
            move |Invocation { ctx, outputs, .. }| { cancel_first = Arc::clone(&cancel_first) } => {
                if cancel_first.swap(false, Ordering::Relaxed) {
                    // Stand in for the user hitting Cancel while this runs.
                    ctx.cancel_flag().cancel();
                }
                outputs[0] = ConstValue::Int(7).into();
                Ok(())
            }
        ))
    });
    let mut e = TestEngine::over(g);

    let run = e
        .run_cancellable(RunSeeds::sinks(), CancelToken::new())
        .await;
    assert!(run.cancelled, "the node cancelled the run mid-invoke");
    assert_eq!(
        run.ran_node_count, 0,
        "an in-flight cancelled node is not reported executed (no green glow)"
    );
    assert!(
        run.status("self_cancel").is_none(),
        "a node the cancel caught mid-invoke reports nothing at all — neither a run \
         nor a failure of its own; the run-level `cancelled` flag is what says why"
    );

    // A fresh token: the partial output was dropped, so the node
    // re-executes rather than being served from a bogus cache.
    let run = e
        .run_cancellable(RunSeeds::sinks(), CancelToken::new())
        .await;
    assert!(!run.cancelled);
    assert_eq!(
        run.ran_node_count, 1,
        "it re-runs; its output was not cached"
    );
    assert!(
        run.cached().is_empty(),
        "a cancelled node must not be served from cache on the next run"
    );
}

/// A lambda that bails by returning `InvokeError::Cancelled` is reported as
/// `RunError::Cancelled` (not a generic `Invoke` error) and dropped from the
/// executed set — the truthful lambda-level signal, distinct from the
/// executor's flag-check fallback covered above (asserted here without
/// touching the flag, so only the error mapping can produce the verdict).
#[tokio::test(flavor = "multi_thread")]
async fn lambda_cancelled_error_maps_to_error_cancelled() {
    use crate::async_lambda;

    let mut g = TestGraph::new();
    g.add("always_cancel", |n| {
        n.pure()
            .sink()
            .output(DataType::Int)
            .lambda(async_lambda!(move |_| { Err(InvokeError::Cancelled) }))
    });
    let mut e = TestEngine::over(g);

    let run = e.run_sinks().await;

    assert_eq!(
        run.ran_node_count, 0,
        "a cancelled lambda is not reported executed"
    );
    assert!(
        run.status("always_cancel").is_none(),
        "InvokeError::Cancelled maps to RunError::Cancelled, which reports nothing — \
         had it mapped to Invoke the node would carry an `Errored` row here"
    );
}

#[tokio::test]
async fn impure_node_always_invoked() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| g.edit_func("get_b", |func| func.behavior = FuncBehavior::Impure));

    // Even holding a cached output, an impure node still wants to execute.
    e.set_output("get_b", vec![ConstValue::Int(7).into()]);
    let plan = e.plan_sinks().await;

    assert_eq!(plan.scheduled(), ["get_b", "get_a", "sum", "mult", "Print"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn impure_output_is_released_after_run() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| g.edit_func("get_b", |func| func.behavior = FuncBehavior::Impure));

    e.run_sinks().await;

    assert!(
        !e.holds_output("get_b"),
        "an impure value cannot hit on a future run, so the end sweep releases it"
    );
}
