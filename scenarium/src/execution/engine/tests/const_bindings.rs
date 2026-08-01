use super::*;

/// The same const value must not re-key the node, and a different one must
/// — checked across four consecutive runs, with the sources wired to
/// `unreachable!` so any walk past the consts fails loudly. The delivered
/// values are pinned alongside the schedule: a const binding must reach the
/// lambda as the authored number, and a re-key must recompute from the new
/// one rather than replay the old product.
#[tokio::test(flavor = "multi_thread")]
async fn const_binding_invokes_only_once() {
    let mut g = TestGraph::sample();
    // A const-fed graph never reaches its sources.
    g.never("get_a");
    g.never("get_b");

    let mut e = TestEngine::over(g);
    e.edit(|g| {
        g.constant("mult", 0, 3i64);
        g.constant("mult", 1, 5i64);
    });

    // The const binds detach mult from its upstream, so get_a/get_b/sum are
    // pruned out of the run entirely.
    assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);
    assert_eq!(e.input_i64("mult", 0), Some(3));
    assert_eq!(e.input_i64("mult", 1), Some(5));
    assert_eq!(e.outputs("mult").len(), 1);
    assert_eq!(e.output_i64("mult", 0), Some(15), "3 * 5");

    // Same const value: no re-execution of mult.
    e.edit(|g| g.constant("mult", 0, 3i64));
    let run = e.run_sinks().await;
    assert_eq!(run.ran(), ["Print"], "mult did not recompute");
    assert!(run.cached().contains(&"mult"), "mult reused");

    // Different const value: mult's digest changes ⇒ cache miss ⇒ re-execute.
    e.edit(|g| g.constant("mult", 0, 4i64));
    assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);
    assert_eq!(e.output_i64("mult", 0), Some(20), "4 * 5");

    // Stable again.
    assert_eq!(e.run_sinks().await.ran(), ["Print"]);
}

/// A const on an input drops the producer that fed it out of the run, and
/// switching back to a bind puts it back — each edit re-keying the consumer.
#[tokio::test(flavor = "multi_thread")]
async fn const_excludes_upstream_node_and_rebinding_restores_it() {
    let mut e = TestEngine::over(TestGraph::sample());
    // Replace sum[0] (get_a) with a const — get_a is no longer needed.
    e.edit(|g| g.constant("sum", 0, 33i64));

    assert_eq!(e.run_sinks().await.ran(), ["get_b", "sum", "mult", "Print"]);

    // Also unbind sum[1]: sum now has all const/none inputs, so no upstream
    // is needed at all.
    e.edit(|g| g.unbind("sum", 1));

    assert_eq!(e.run_sinks().await.ran(), ["sum", "mult", "Print"]);

    // Switch sum[0] from const back to a bind — sum must re-execute, and its
    // producer is served from the value it cached two runs ago rather than
    // re-run for having sat out the runs in between.
    e.edit(|g| g.wire("get_b", 0, "sum", 0));

    assert_eq!(e.run_sinks().await.ran(), ["sum", "mult", "Print"]);
}

/// Changing which *kind* of binding an input carries re-keys the consumer
/// each time: bind → const/none re-executes it, as does const → bind. A
/// producer that has already been computed feeds the recompute from cache.
#[tokio::test(flavor = "multi_thread")]
async fn input_binding_change_recomputes_and_reuses_cached_upstream() {
    let mut e = TestEngine::over(TestGraph::sample());
    e.run_sinks().await;

    // Switch mult's inputs to const/none: its upstream leaves the run.
    e.edit(|g| {
        g.constant("mult", 0, 2i64);
        g.unbind("mult", 1);
    });
    assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);

    // Stable on rerun.
    assert_eq!(e.run_sinks().await.ran(), ["Print"]);

    // Switch mult[0] back to a bind from the *cached* get_b — mult
    // re-executes, but its producer is served from cache rather than re-run.
    e.edit(|g| g.wire("get_b", 0, "mult", 0));
    assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);
}
