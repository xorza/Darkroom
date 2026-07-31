use super::*;

/// The sample fixture with nothing retained — every node on
/// `CacheMode::None`, which is what these tests are about.
fn uncached() -> TestGraph {
    let mut g = TestGraph::sample();
    g.cache_all(CacheMode::None);
    g
}

/// Seeding `sum` runs exactly its cone (`get_a`, `get_b`, `sum`) without
/// overriding any node's `CacheMode::None` retention policy.
#[tokio::test]
async fn seeded_run_executes_only_the_cone_without_retaining_outputs() {
    let mut g = uncached();
    // Nothing downstream of `sum` may be reached by a seed on `sum`.
    g.never("mult");
    g.never("Print");
    let mut e = TestEngine::over(g);

    let run = e.run_nodes(["sum"]).await;

    assert_eq!(run.ran(), ["get_b", "get_a", "sum"], "only sum's cone runs");
    assert_eq!(run.ran_node_count, 3);
    assert!(run.holding_ram().is_empty());
    assert_eq!(run.cache_ram.total(), 0);
    assert!(
        e.outputs("sum").is_empty(),
        "the targeted output is produced but not retained"
    );
    assert!(
        e.outputs("get_a").is_empty(),
        "unpinned None-cache upstream is drained as usual"
    );
}

/// A second seeded run obeys `CacheMode::None` and recomputes the cone.
#[tokio::test]
async fn second_seeded_run_obeys_none_cache_mode() {
    // One counter behind both sources, so it reads as "how many source
    // invocations did this run cost".
    let calls = Calls::default();
    let mut g = TestGraph::new();
    g.add("a", |n| n.counted(1i64, &calls));
    g.add("b", |n| n.counted(11i64, &calls));
    g.add("sum", |n| n.sum());
    g.wire("a", 0, "sum", 0);
    g.wire("b", 0, "sum", 1);
    g.cache_all(CacheMode::None);
    let mut e = TestEngine::over(g);

    e.run_nodes(["sum"]).await;
    assert_eq!(calls.count(), 2, "one call to each source");

    let run = e.run_nodes(["sum"]).await;

    assert_eq!(
        calls.count(),
        4,
        "nothing was retained, so both sources ran again"
    );
    assert_eq!(run.ran_node_count, 3);
    assert!(!run.cached().contains(&"sum"));
    assert!(run.holding_ram().is_empty());
}

/// Node seeds combine with a sink run without retaining `CacheMode::None`
/// values — and the seed overrides `disabled` for that run.
#[tokio::test]
async fn node_seed_combines_with_a_sink_run_without_retaining() {
    let mut e = TestEngine::over(uncached());
    e.edit(|g| g.disable("sum"));

    let run = e
        .run(RunSeeds {
            sinks: true,
            node_ids: vec![e.id("sum")],
            ..Default::default()
        })
        .await;

    assert_eq!(
        run.ran(),
        ["get_b", "get_a", "sum", "mult", "Print"],
        "the explicit override feeds the ordinary sink during this run"
    );
    assert_eq!(run.logs(), ["132"], "(1 + 11) * 11");
    assert_eq!(e.output_i64("mult", 0), None, "…and it is not retained");
    assert!(
        e.outputs("sum").is_empty(),
        "the targeted value is released after its real consumer"
    );
    assert!(
        e.outputs("mult").is_empty(),
        "the None-cache downstream is drained by its consumer"
    );
    assert!(run.holding_ram().is_empty());
}

/// A seed that doesn't resolve against the compiled program (deleted or
/// stale node) fails the run because the miss is inconsistent caller state,
/// not something to silently skip. Every body panics if reached, so the
/// failure is proven to happen before any of them fires.
#[tokio::test]
async fn unresolvable_node_seed_fails_the_run() {
    let mut g = TestGraph::sample();
    g.never_all();
    let mut e = TestEngine::over(g);

    let bogus = NodeId::from_u128(0xdead_beef);
    let error = e
        .try_run(RunSeeds::nodes(vec![bogus]))
        .await
        .expect_err("a stale seed fails the run");

    assert!(matches!(error, Error::NodeSeedNotFound { node_id } if node_id == bogus));
}
