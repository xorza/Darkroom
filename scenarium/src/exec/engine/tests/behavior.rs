use super::*;

use std::sync::atomic::{AtomicBool, Ordering};

#[tokio::test]
async fn execute_emits_started_then_succeeded_progress_per_node() {
    use crate::exec::report::run_reporter::RunPhase;

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
            matches!(finished_phase, RunPhase::Succeeded { elapsed_secs } if *elapsed_secs >= 0.0),
            "second of pair is a success with non-negative elapsed",
        );
        started.push(started_name);
    }

    // The progressed order equals the run's own order, and covers exactly
    // the nodes that finally executed.
    assert_eq!(started, run.ran());
    assert_eq!(started.len(), run.executed_node_count);
}

/// A lambda that fails reports its finish as a failure, so a host never paints it executed; the
/// consumers it starves never start.
#[tokio::test]
async fn a_failing_node_finishes_as_a_failure() {
    use crate::exec::report::run_reporter::RunPhase;

    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| g.fails("sum", "boom"));
    let ReportedRun { progress, .. } = e.run_sinks_reporting().await;
    let sum: Vec<RunPhase> = progress
        .iter()
        .filter(|(name, _)| *name == "sum")
        .map(|&(_, phase)| phase)
        .collect();
    assert!(
        matches!(
            sum.as_slice(),
            [RunPhase::Started { .. }, RunPhase::Failed { .. }]
        ),
        "{sum:?}"
    );
    assert!(progress.iter().all(|(name, _)| *name != "mult"));
}

#[tokio::test]
async fn execute_honors_cancel_flag_and_marks_cancelled() {
    let mut e = TestEngine::over(TestGraph::sample());

    // Pre-tripped: the executor breaks at the first loop-top check, so no
    // node runs and the run is flagged cancelled.
    let tripped = CancelToken::new();
    tripped.cancel();
    let run = e.run_cancellable(RunSeeds::sinks(), tripped).await;
    assert!(run.cancelled, "pre-tripped run is cancelled");
    assert_eq!(
        run.executed_node_count, 0,
        "no node runs when cancel is set"
    );

    // A fresh token runs the whole graph — the aborted run cached nothing.
    let run = e
        .run_cancellable(RunSeeds::sinks(), CancelToken::new())
        .await;
    assert!(!run.cancelled);
    assert_eq!(
        run.executed_node_count, 5,
        "all nodes run when not cancelled"
    );
}

/// A node cancelled *mid-invoke* (the run is cancelled while its lambda
/// runs) must not be reported executed and must not cache its partial
/// output — otherwise the next run treats it as already computed. Models
/// "start a run, immediately cancel it": the in-flight node bails with `Ok`
/// but its result is bogus.
#[tokio::test]
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
        run.executed_node_count, 0,
        "an in-flight cancelled node is not reported executed (no green glow)"
    );
    assert!(
        run.status("self_cancel").is_none(),
        "a node the cancel caught mid-invoke reports nothing at all — neither a run \
         nor a failure of its own; the run-level `cancelled` flag is what says why"
    );

    // A fresh token: the partial output was dropped, so the node re-executes.
    let run = e
        .run_cancellable(RunSeeds::sinks(), CancelToken::new())
        .await;
    assert!(!run.cancelled);
    assert_eq!(
        run.executed_node_count, 1,
        "it re-runs; its output was not cached"
    );
    assert!(
        run.cached().is_empty(),
        "a cancelled node must not be served from cache on the next run"
    );
}

/// A lambda that bails by returning `InvokeError::Cancelled` is reported as `RunError::Cancelled`
/// (not a generic `Invoke` error) and dropped from the executed set — the truthful lambda-level
/// signal, distinct from the executor's flag-check fallback covered above (asserted without
/// touching the flag, so only the error mapping can produce the verdict).
#[tokio::test]
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
        run.executed_node_count, 0,
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

#[tokio::test]
async fn impure_output_is_released_after_run() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| g.edit_func("get_b", |func| func.behavior = FuncBehavior::Impure));

    e.run_sinks().await;

    assert!(
        !e.holds_output("get_b"),
        "an impure value cannot hit on a future run, so the end sweep releases it"
    );
}

/// A pure sink is a display: it runs when what it reads changed, and not again on the same
/// inputs. Two pure sinks each read an uncached source: the first run runs all four; a second
/// runs none, each sink served from the digest it last ran under and its source not pulled; a
/// constant changed on one source runs that source and its sink alone, and a seed of the other
/// sink, a "run to this node", still serves it.
#[tokio::test]
async fn a_pure_sink_runs_once_per_input_digest() {
    let mut g = TestGraph::new();
    for (source, sink) in [("left", "show_left"), ("right", "show_right")] {
        g.add(source, NodeSpec::sum);
        g.constant(source, 0, 1i64);
        g.add(sink, |n| {
            n.pure().sink().input(DataType::Int).observes(|_| {})
        });
        g.wire(source, sink);
    }
    let mut e = TestEngine::over(g);
    let sorted = |run: &RunOutcome| {
        let mut ran = run.ran();
        ran.sort_unstable();
        ran.into_iter().map(str::to_owned).collect::<Vec<_>>()
    };

    let run = e.run_sinks().await;
    assert_eq!(sorted(&run), ["left", "right", "show_left", "show_right"]);
    let run = e.run_sinks().await;
    assert!(run.ran().is_empty(), "{:?}", run.ran());
    assert_eq!(run.cached(), ["show_left", "show_right"]);

    e.edit(|g| g.constant("left", 0, 2i64));
    let run = e.run_sinks().await;
    assert_eq!(sorted(&run), ["left", "show_left"]);
    let seeded = e.run_nodes(["show_right"]).await;
    assert!(seeded.ran().is_empty(), "{:?}", seeded.ran());
}

/// A node kept while memory allows, every node's default outside a fixture, is served on the next
/// run its digest holds, and each run stamps the nodes it ran or served with its count, which
/// memory pressure drops the least recently used by: `src` runs in run 1 and is served in run 2,
/// stamped 1 then 2, while the impure `Print` runs in both.
#[tokio::test]
async fn a_node_kept_while_memory_allows_is_served_and_stamped_by_its_run() {
    let mut g = TestGraph::new();
    g.add("src", |n| n.sum().cache(CacheMode::AUTO));
    g.constant("src", 0, 1i64);
    g.add("Print", NodeSpec::records);
    g.wire("src", "Print");
    let mut e = TestEngine::over(g);

    assert_eq!(e.run_sinks().await.ran(), ["src", "Print"]);
    assert_eq!(e.engine.slot(e.id("src")).used_in, 1);
    let run = e.run_sinks().await;
    assert_eq!(run.ran(), ["Print"]);
    assert_eq!(run.cached(), ["src"]);
    assert_eq!(e.engine.slot(e.id("src")).used_in, 2);
}

/// Under memory pressure a run drops each value kept while memory allows once nothing later in
/// it reads the value, and keeps what is read and what is kept always: with no memory left, `src`
/// (a 64-byte blob) survives until `mid` reads it and `mid` (its 64 bytes and 64 more) until
/// `Print` does, so `Print` sees 128 bytes; after the run both are gone and run again, while
/// `pinned`, kept always, is served.
#[tokio::test]
async fn memory_pressure_drops_what_no_later_node_reads() {
    use crate::async_lambda;
    use crate::data::data_type::custom_type::CustomType;
    use crate::internals::blob::{BLOB_TYPE, Blob};

    let blob = |inputs: &[DynamicValue]| -> Vec<u8> {
        inputs
            .first()
            .and_then(|value| value.as_custom::<Blob>())
            .map_or_else(Vec::new, |blob| blob.0.clone())
    };
    let grows = move |cache| {
        move |n: NodeSpec| {
            n.pure()
                .optional(DataType::Custom(BLOB_TYPE))
                .output(DataType::Custom(BLOB_TYPE))
                .cache(cache)
                .lambda(async_lambda!(
                    move |Invocation {
                              inputs, outputs, ..
                          }| {
                        let mut bytes = blob(inputs);
                        bytes.extend([0; 64]);
                        outputs[0] = DynamicValue::Custom(Arc::new(Blob(bytes)));
                        Ok(())
                    }
                ))
        }
    };
    let mut g = TestGraph::new();
    g.library
        .register_custom_type(BLOB_TYPE, CustomType::new("Blob"));
    g.add("src", grows(CacheMode::AUTO));
    g.add("mid", grows(CacheMode::AUTO));
    g.wire("src", "mid");
    g.add("Print", |n| {
        n.sink().input(DataType::Any).lambda(async_lambda!(
            move |Invocation { ctx, inputs, .. }| {
                ctx.info(blob(inputs).len().to_string());
                Ok(())
            }
        ))
    });
    g.wire("mid", "Print");
    g.add("pinned", grows(CacheMode::ALWAYS));
    g.add("Print pinned", NodeSpec::records);
    g.wire("pinned", "Print pinned");
    let mut e = TestEngine::over(g);
    // 512 B: memory is read after every 512 / 8 = 64 B a run makes, so after each node here.
    e.engine.set_ram_headroom(512);
    e.engine.set_memory_probe(|| 0);

    let run = e.run_sinks().await;
    assert!(run.logs().contains(&"128"), "{:?}", run.logs());
    assert!(e.engine.slot(e.id("src")).output_values().is_none());
    assert!(e.engine.slot(e.id("mid")).output_values().is_none());
    assert!(e.engine.slot(e.id("pinned")).output_values().is_some());

    let run = e.run_sinks().await;
    let mut ran = run.ran();
    ran.sort_unstable();
    assert_eq!(ran, ["Print", "Print pinned", "mid", "src"]);
}
