use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn const_binding_tracks_changes() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| {
        g.constant("mult", 0, 3i64);
        g.constant("mult", 1, 5i64);
    });

    // The const binds detach mult from its upstream, so get_a/get_b/sum are
    // pruned out of the run entirely.
    let run = e.run_sinks().await;
    assert_eq!(run.ran(), ["mult", "Print"]);

    // Re-run with the same bindings: mult's digest is unchanged, so it is
    // reused; only print (an impure sink) recomputes.
    let run = e.run_sinks().await;
    assert_eq!(run.ran(), ["Print"], "mult did not recompute");
    assert!(run.cached().contains(&"mult"), "mult reused");

    // Change one const: mult's digest changes ⇒ cache miss ⇒ it re-executes.
    e.edit(|g| g.constant("mult", 0, 4i64));
    let run = e.run_sinks().await;
    assert_eq!(run.ran(), ["mult", "Print"]);
    Ok(())
}

/// The same const value must not re-key the node, and a different one must
/// — checked across four consecutive runs, with the sources wired to
/// `unreachable!` so any walk past the consts fails loudly.
#[tokio::test(flavor = "multi_thread")]
async fn const_binding_invokes_only_once() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample_with(TestFuncHooks {
        get_a: Arc::new(|| unreachable!("a const-fed graph never reaches its sources")),
        get_b: Arc::new(|| unreachable!("a const-fed graph never reaches its sources")),
        print: Arc::new(|_| {}),
    }));
    e.edit(|g| {
        g.constant("mult", 0, 3i64);
        g.constant("mult", 1, 5i64);
    });

    assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);

    // Same const value: no re-execution of mult.
    e.edit(|g| g.constant("mult", 0, 3i64));
    assert_eq!(e.run_sinks().await.ran(), ["Print"]);

    // Different const value: mult re-executes.
    e.edit(|g| g.constant("mult", 0, 4i64));
    assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);

    // Stable again.
    assert_eq!(e.run_sinks().await.ran(), ["Print"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn const_excludes_upstream_node() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample());
    // Replace sum[0] (get_a) with a const — get_a is no longer needed.
    e.edit(|g| g.constant("sum", 0, 33i64));

    assert_eq!(e.run_sinks().await.ran(), ["get_b", "sum", "mult", "Print"]);

    // Also unbind sum[1]: sum now has all const/none inputs, so no upstream
    // is needed at all.
    e.edit(|g| g.unbind("sum", 1));

    assert_eq!(e.run_sinks().await.ran(), ["sum", "mult", "Print"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn change_from_const_to_bind_recomputes() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample());
    e.edit(|g| g.constant("sum", 0, 33i64));

    assert_eq!(e.run_sinks().await.ran(), ["get_b", "sum", "mult", "Print"]);

    // Switch from const back to a bind — sum must re-execute.
    e.edit(|g| g.wire("get_b", 0, "sum", 0));

    assert_eq!(e.run_sinks().await.ran(), ["sum", "mult", "Print"]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn optional_input_binding_change_recomputes() -> TestResult {
    let mut e = TestEngine::over(TestGraph::sample());
    e.run_sinks().await;

    // Switch mult's inputs to const/none.
    e.edit(|g| {
        g.constant("mult", 0, 2i64);
        g.unbind("mult", 1);
    });
    assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);

    // Stable on rerun.
    assert_eq!(e.run_sinks().await.ran(), ["Print"]);
    Ok(())
}
