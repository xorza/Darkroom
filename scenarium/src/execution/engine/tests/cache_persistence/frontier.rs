use super::*;

#[tokio::test]
async fn explicit_cache_eviction_removes_the_downstream_ram_and_disk_cone() {
    let dir = TempDir::new("explicit-eviction");
    let (a_calls, b_calls) = (Calls::default(), Calls::default());

    // src_a → sum → mult → print, with src_b feeding sum's second input —
    // an upstream sibling *outside* src_a's consumer cone.
    let mut g = TestGraph::new();
    g.add("src_a", |n| {
        n.counted(1i64, &a_calls).cache(CacheMode::Both)
    });
    g.add("src_b", |n| {
        n.counted(11i64, &b_calls).cache(CacheMode::Both)
    });
    g.add("sum", |n| n.sum().cache(CacheMode::Both));
    g.add("mult", |n| n.mult().cache(CacheMode::Both));
    g.add("print", |n| n.records());
    g.wire("src_a", 0, "sum", 0);
    g.wire("src_b", 0, "sum", 1);
    g.wire("sum", 0, "mult", 0);
    g.wire("src_b", 0, "mult", 1);
    g.wire("mult", 0, "print", 0);

    let mut e = TestEngine::over(g).with_disk_store(dir.path());
    e.run_sinks().await;
    let run = e.run_sinks().await;
    assert_eq!(a_calls.count(), 1);
    assert_eq!(b_calls.count(), 1);
    assert_eq!(run.logs(), ["132"], "(1 + 11) * 11");
    assert_eq!(dir.entry_count(), 4);

    // Evicting the source takes its whole consumer cone with it — evicting
    // one node alone would free a slot and change nothing a later run does,
    // since a surviving consumer would just reuse and prune it again.
    let evicted = ["src_a", "sum", "mult", "print"];
    assert!(
        e.evict(["src_a"]).await.is_empty(),
        "the selected source and its data consumers must all evict"
    );
    for name in evicted {
        assert!(
            !e.holds_output(name),
            "{name} must release its resident output"
        );
    }
    assert!(
        e.holds_output("src_b"),
        "an upstream sibling outside the consumer cone stays resident"
    );
    assert_eq!(dir.entry_count(), 1, "only src_b's disk blob remains");

    // Reopening recomputes exactly what was evicted; the retained sibling
    // is still served from its blob.
    let mut e = e.reopen();
    let run = e.run_sinks().await;
    assert_eq!(a_calls.count(), 2);
    assert_eq!(b_calls.count(), 1);
    assert_eq!(run.logs(), ["132"]);
    for name in evicted {
        assert!(
            run.ran().contains(&name),
            "{name} must recompute after reopening"
        );
    }
    assert!(
        !run.ran().contains(&"src_b"),
        "the retained sibling blob must still be reusable after reopening"
    );

    // A blob that cannot be deleted is reported, and the rest of the cone
    // still evicts — one failure is not a reason to abandon the sweep.
    let blocked = e.blob_path("src_a");
    std::fs::remove_file(&blocked).unwrap();
    std::fs::create_dir(&blocked).unwrap();

    let failures = e.evict(["src_a"]).await;
    let [failure] = failures.as_slice() else {
        panic!("the undeletable src_a path must be the only eviction failure");
    };
    assert_eq!(failure.node_id, e.id("src_a"));
    let CacheNodeError::Removal(error) = &failure.cause else {
        panic!(
            "an undeletable blob is a removal failure, got {:?}",
            failure.cause
        );
    };
    assert_eq!(error.path, blocked, "the failure names the blob it kept");
    assert!(
        e.holds_output("src_a"),
        "a failed disk deletion must leave the matching RAM value resident"
    );
    for name in ["sum", "mult", "print"] {
        assert!(
            !e.holds_output(name),
            "{name} must still evict when another target fails"
        );
    }
}

/// Fan-out: a producer feeding both a reuse hit *and* a running consumer
/// must survive the cut — the running consumer still reads it. Proves the
/// cut is a backward union over consumers, not a forward "all consumers
/// reused" filter (which would wrongly prune the shared producer and starve
/// the executing branch).
#[tokio::test]
async fn shared_producer_read_by_a_running_consumer_is_not_cut() {
    let dir = TempDir::new("fanout");
    let calls = Calls::default();

    // src → mult(Disk) → print_mult ;  src → print_direct.
    let mut g = TestGraph::new();
    g.add("src", |n| n.counted(7i64, &calls));
    g.add("mult", |n| n.mult().cache(CacheMode::Disk));
    g.add("print_mult", |n| n.records());
    g.instance("print_direct", "print_mult");
    g.wire("src", 0, "mult", 0);
    g.wire("src", 0, "mult", 1);
    g.wire("mult", 0, "print_mult", 0);
    g.wire("src", 0, "print_direct", 0);

    let mut e = TestEngine::over(g).with_disk_store(dir.path());
    e.run_sinks().await;
    assert_eq!(calls.count(), 1);

    // Reopen: mult reuses from disk, so the src→mult edge is cut — but
    // print_direct still reads src, so the union keeps src alive.
    let mut e = e.reopen();
    let run = e.run_sinks().await;
    assert_eq!(
        calls.count(),
        2,
        "src is still read by print_direct, so the cut must keep it"
    );
    assert!(
        run.ran().contains(&"src"),
        "the shared producer runs for its executing consumer"
    );
    assert!(
        run.cached().contains(&"mult"),
        "mult still reuses from disk"
    );
}

/// Two disk-cached nodes chained (`sum` → `mult`) under an executing sink.
/// On reopen only the frontier `mult` — the cached value the sink actually
/// reads — is deserialized into RAM; the deeper `sum`, whose sole consumer
/// is itself reused-from-disk, is never hydrated.
#[tokio::test]
async fn chained_disk_cache_hydrates_only_the_live_frontier() {
    let dir = TempDir::new("chain-frontier");
    let calls = Calls::default();

    // src(7) → sum(Both) = 14 → mult(Both) = 98 → print. `Both` (RAM + disk)
    // so the frontier the run reads is kept resident — that retention is
    // what this test asserts; pure `Disk` would drop its RAM copy.
    let mut g = TestGraph::new();
    g.add("src", |n| n.counted(7i64, &calls));
    g.add("sum", |n| n.sum().cache(CacheMode::Both));
    g.add("mult", |n| n.mult().cache(CacheMode::Both));
    g.add("print", |n| n.records());
    g.wire("src", 0, "sum", 0);
    g.wire("src", 0, "sum", 1);
    g.wire("sum", 0, "mult", 0);
    g.wire("src", 0, "mult", 1);
    g.wire("mult", 0, "print", 0);

    let mut e = TestEngine::over(g).with_disk_store(dir.path());
    e.run_sinks().await;
    assert_eq!(calls.count(), 1);

    // Reopen with fresh RAM. Resolution alone settles `mult` as a reuse —
    // its blob header covers the demand — without decoding the body: the
    // value only enters RAM when the run loop reaches the node, so a run's
    // reusable frontier never accumulates ahead of the first lambda.
    let mut e = e.reopen();
    e.plan_sinks().await;
    assert_eq!(
        e.state("mult"),
        NodeState::Reuse,
        "the frontier blob is verified from its header during resolution"
    );
    assert!(!e.holds_output("mult"), "...and is not decoded there");

    let run = e.run_sinks().await;

    assert_eq!(
        calls.count(),
        1,
        "the cut prunes the memory-only source feeding only disk hits"
    );
    assert_eq!(
        run.cached(),
        ["mult"],
        "only the live frontier cache is hydrated and reported"
    );
    assert!(e.holds_output("mult"), "frontier cache is loaded into RAM");
    assert!(
        !e.holds_output("sum"),
        "an unneeded upstream disk cache is never hydrated — the blob stays \
         in the store until a later run's exact demand reaches it"
    );

    // Swap in an empty store: the resident frontier survives, but the deeper
    // value now has nothing to load from and must recompute when demanded.
    let empty = TempDir::new("chain-empty");
    e.attach_disk_store(empty.path());
    assert!(
        e.holds_output("mult"),
        "switching stores preserves resident values"
    );

    e.edit(|g| g.constant("mult", 1, 3i64));
    let run = e.run_sinks().await;
    assert!(
        run.ran().contains(&"sum"),
        "a value absent from the new store recomputes when needed"
    );
    assert_eq!(
        calls.count(),
        2,
        "recomputing sum also restores its pruned memory-only input"
    );
}

/// A valid disk blob for a node's *current* digest must be served even when
/// the slot still holds a RAM value produced under a superseded digest — the
/// stale resident value must not mask the fresh blob. Disk reuse must load
/// the current blob before deciding an older resident value is reusable.
///
/// The intervening run uses `Ram` mode so it can't overwrite the node's one
/// disk blob (a `Disk`-mode run would — the blob is keyed by node id).
#[tokio::test(flavor = "multi_thread")]
async fn stale_ram_value_does_not_mask_a_valid_disk_blob() {
    let dir = TempDir::new("flip_back");

    // Config A (Disk): mult = 2 * 3 = 6 → the blob (digest D_A) on disk.
    // Const binds detach mult from any upstream, so its digest is a pure
    // function of the two consts.
    let mut g = TestGraph::new();
    g.add("mult", |n| n.mult().cache(CacheMode::Disk));
    g.add("print", |n| n.records());
    g.constant("mult", 0, 2i64);
    g.constant("mult", 1, 3i64);
    g.wire("mult", 0, "print", 0);

    let mut e = TestEngine::over(g).with_disk_store(dir.path());
    let first = e.run_sinks().await;
    assert_eq!(first.logs(), ["6"]);

    // Config B (Ram): mult = 5 * 7 = 35 → the slot is now resident with 35
    // under B's digest; the disk blob still carries D_A, since Ram never
    // writes. The same engine keeps the slot across the reinstall.
    e.edit(|g| {
        g.constant("mult", 0, 5i64);
        g.constant("mult", 1, 7i64);
        g.cache("mult", CacheMode::Ram);
    });
    let second = e.run_sinks().await;
    assert_eq!(second.logs(), ["35"]);

    // Flip back to A with Both, so the install preserves the current B
    // snapshot in RAM. Resolution then stamps A's digest, making 35
    // superseded before disk reuse probes the matching A blob — it must
    // serve 6 from disk, not the stale 35.
    e.edit(|g| {
        g.constant("mult", 0, 2i64);
        g.constant("mult", 1, 3i64);
        g.cache("mult", CacheMode::Both);
    });
    assert_eq!(
        e.output_i64("mult", 0),
        Some(35),
        "the stale B snapshot remains resident when the flip-back run begins"
    );

    let run = e.run_sinks().await;
    assert!(
        !run.ran().contains(&"mult"),
        "mult is a disk cache hit on flip-back, not recomputed — without \
         this a recompute would yield 6 regardless and the stale-RAM path \
         would go untested"
    );
    assert_eq!(
        run.logs(),
        ["6"],
        "flip-back serves the disk blob (6), not the stale RAM value (35)"
    );
}

/// A persisted node is written to disk the moment *it* finishes, not in a
/// batch at the end of the run — so its blob is already on disk by the time
/// a downstream node executes. The sink checks the store dir is non-empty
/// when it runs; that holds only because `mult` was persisted right after it
/// finished. Batched-at-the-end storing would leave the dir empty here.
#[tokio::test(flavor = "multi_thread")]
async fn persist_node_lands_on_disk_before_its_consumer_runs() {
    let dir = TempDir::new("per_node_store");
    let root = dir.path().to_path_buf();
    let blob_present = Arc::new(AtomicBool::new(false));

    // mult(const 2, const 3) = 6, Disk → sink. Const binds detach mult from
    // any upstream, so only mult and the sink run.
    let mut g = TestGraph::new();
    g.add("mult", |n| n.mult().cache(CacheMode::Disk));
    g.add("watch", |n| {
        let (root, flag) = (root.clone(), blob_present.clone());
        n.sink().input(DataType::Int).observes(move |_| {
            let non_empty = std::fs::read_dir(&root)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false);
            flag.store(non_empty, Ordering::SeqCst);
        })
    });
    g.constant("mult", 0, 2i64);
    g.constant("mult", 1, 3i64);
    g.wire("mult", 0, "watch", 0);

    let mut e = TestEngine::over(g).with_disk_store(dir.path());
    e.run_sinks().await;

    assert!(
        blob_present.load(Ordering::SeqCst),
        "mult's disk blob must exist when its consumer runs (persisted \
         per-node, not batched at the end of the run)"
    );
}

/// A flush must not write a value under a digest it wasn't produced under.
/// After an input change recompiles the program, a node's resident value is
/// stale w.r.t. its new digest; flushing it stamped with D_B would overwrite
/// the node's blob with bytes a later run at D_B would load as a false hit.
///
/// `Both`, so the value is still resident when the flush reaches it, and a
/// plan between the edit and the flush, so the slot's *current* digest really
/// has moved to D_B — under `Disk` there would be no resident value to
/// mis-stamp, and with no plan the slot would still be carrying D_A, either of
/// which leaves the flush nothing to get wrong.
#[tokio::test]
async fn flush_skips_a_value_stale_for_the_current_digest() {
    let dir = TempDir::new("stale_flush");

    let mut g = TestGraph::new();
    g.add("mult", |n| n.mult().cache(CacheMode::Both));
    g.add("print", |n| n.records());
    g.constant("mult", 0, 2i64);
    g.constant("mult", 1, 3i64);
    g.wire("mult", 0, "print", 0);

    // Config A: mult runs and is stored, stamped with its digest D_A.
    let mut e = TestEngine::over(g).with_disk_store(dir.path());
    e.run_sinks().await;
    assert_eq!(dir.entry_count(), 1, "config A's blob is stored");
    let blob_a = e.blob("mult");

    // Config B: mult's inputs change ⇒ its *current* digest is now D_B, but
    // the resident value was produced under D_A. Recompile and re-plan, do
    // not re-run, then flush — the stale value must not be re-stamped D_B.
    // The blob is keyed by node id, so a bad flush shows as an overwrite.
    e.edit(|g| {
        g.constant("mult", 0, 5i64);
        g.constant("mult", 1, 7i64);
    });
    e.plan_sinks().await;
    assert!(
        e.holds_output("mult"),
        "the stale value is still resident, so the flush has one to mis-stamp"
    );
    e.flush_all().await;
    assert_eq!(
        e.blob("mult"),
        blob_a,
        "a value stale for the current digest is not flushed (blob untouched)"
    );

    // The same flush over the node named directly, rather than the whole
    // program: the skip is the value's, not the walk's.
    e.flush(["mult"]).await;
    assert_eq!(e.blob("mult"), blob_a, "the per-node flush skips it too");
}
