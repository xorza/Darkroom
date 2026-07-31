use super::*;

/// A disk-persisted node's output survives a fresh engine (reopen), its
/// sole-consumer upstream is pruned on the hit, and an input change
/// invalidates it — *overwriting* the node's one blob rather than orphaning
/// it beside a new one.
#[tokio::test]
async fn persist_output_survives_reopen_and_invalidates_on_digest_change() {
    let dir = TempDir::new("e2e");
    let calls = Calls::default();
    let build = |calls: &Calls| source_mult_print(CacheMode::Disk, 7, calls);

    // First run: everything computes; `mult` is stored to disk.
    let mut e = disk_engine(&dir, build(&calls));
    e.run_sinks().await;
    assert_eq!(calls.count(), 1);

    // Reopen: `mult` loads from disk. Its only consumer of `src` is the
    // reused `mult`, which never reads it, so the pre-run cut prunes `src`
    // — a RAM-only source with no cross-session cache is *not* recomputed.
    let mut e = disk_engine(&dir, build(&calls));
    let run = e.run_sinks().await;
    assert_eq!(
        calls.count(),
        1,
        "the cut prunes the memory-only source upstream of a disk hit"
    );
    assert!(!run.ran().contains(&"src"), "src was cut, not executed");
    assert!(run.cached().contains(&"mult"), "mult reused from disk");
    assert!(!run.ran().contains(&"mult"), "mult did not recompute");
    assert!(
        !e.holds_output("mult"),
        "a full run does not retain a Disk node after the run"
    );

    // A targeted run on `mult` hydrates the disk hit, but targeting must not
    // turn it into an implicit RAM cache.
    let run = e.run_nodes(["mult"]).await;
    assert!(run.cached().contains(&"mult"));
    assert!(
        !e.holds_output("mult"),
        "a targeted run releases the hydrated Disk value"
    );

    // Changing one input to a const makes `mult` miss, while its other input
    // still needs `src`, so the cut keeps the source alive and it runs.
    let mut e = disk_engine(&dir, build(&calls));
    e.edit(|g| g.constant("mult", 1, 3i64));
    let run = e.run_sinks().await;
    assert_eq!(
        calls.count(),
        2,
        "an input change makes mult miss and recompute from src"
    );
    assert!(
        !run.cached().contains(&"mult"),
        "mult should not be cached after a digest change"
    );
    // The blob is keyed by node id, so the recompute replaced the superseded
    // bytes in place — the old digest's cache doesn't linger as an orphan.
    assert_eq!(
        dir.entry_count(),
        1,
        "a digest change overwrites the node's blob, not adds a second"
    );
}

/// A blob that satisfies the resolver's header probe but fails to decode
/// when the run loop reaches it. The reuse verdict already cut the node's
/// producers, so the run cannot fall back to recomputing: the node fails,
/// its consumers skip as errored-upstream, and the undecodable blob is
/// dropped so the next run recomputes.
#[tokio::test]
async fn a_probed_blob_that_stops_decoding_fails_its_node_and_self_heals() {
    let dir = TempDir::new("corrupt-frontier");
    let calls = Calls::default();
    let build = |calls: &Calls| source_mult_print(CacheMode::Disk, 7, calls);

    let mut e = disk_engine(&dir, build(&calls));
    e.run_sinks().await;
    assert_eq!(calls.count(), 1);

    // Reopen, then corrupt the stored value while leaving the header the
    // probe reads — digest, arity, per-output coverage — untouched.
    let mut e = disk_engine(&dir, build(&calls));
    e.engine.cache.disk_store().corrupt_payload(e.id("mult"), 1);
    e.plan_sinks().await.unwrap();
    assert_eq!(
        e.state("mult"),
        NodeState::Reuse,
        "a header-only probe cannot see a corrupt payload"
    );

    let run = e.run_sinks().await;
    assert_eq!(
        calls.count(),
        1,
        "the reuse verdict already pruned the producer, so nothing recomputes"
    );
    assert!(
        matches!(run.error("mult"), Some(RunError::CacheLoadFailed { .. })),
        "the node whose cache stopped loading fails, rather than serving nothing"
    );
    assert!(
        matches!(run.error("print"), Some(RunError::SkippedUpstream { .. })),
        "its consumer skips as errored-upstream"
    );
    assert!(!run.cached().contains(&"mult"));
    assert_eq!(
        dir.entry_count(),
        0,
        "the undecodable blob is dropped rather than left to fail every future run"
    );

    // Nothing left to reuse: the whole cone recomputes and republishes.
    let run = e.run_sinks().await;
    assert!(run.errored().is_empty(), "the next run is clean");
    assert_eq!(calls.count(), 2);
    assert_eq!(dir.entry_count(), 1);
}

/// A corrupt / incompatible cache blob must be *deleted* on a failed load so
/// the same run recomputes and writes a fresh one. Without the delete,
/// `store_node`'s skip-if-exists keeps the broken file and the node
/// recomputes on *every* run — the regression being an old-format blob
/// rejected by the outer format version and never replaced. Each session is
/// a fresh engine, so the disk cache is the only source.
#[tokio::test]
async fn corrupt_blob_recomputes_and_is_replaced_in_the_same_run() {
    let dir = TempDir::new("corrupt_replace");
    let calls = Calls::default();
    let build = |calls: &Calls| source_mult_print(CacheMode::Disk, 1, calls);

    // Cold run: mult computes and stores its blob.
    {
        let mut e = disk_engine(&dir, build(&calls));
        let run = e.run_sinks().await;
        assert!(run.ran().contains(&"mult"), "the cold run computes mult");
    }
    assert_eq!(calls.count(), 1);

    // Corrupt mult's blob *body* — a torn write, or an old
    // version-mismatched format — while keeping the leading 32-byte digest
    // header intact: a garbled header would already fail the presence probe
    // and never reach the body verification this test is about.
    let blob = std::fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut bytes = std::fs::read(&blob).unwrap();
    let output_count = u32::from_le_bytes(bytes[36..40].try_into().unwrap()) as usize;
    bytes.truncate(40 + output_count);
    bytes.extend_from_slice(b"garbage");
    std::fs::write(&blob, &bytes).unwrap();

    // Reopen: the corrupt blob still carries the current digest in its
    // header. Body verification fails before the resolver cuts the producer
    // cone, so the blob is deleted and mult recomputes in this same run.
    {
        let mut e = disk_engine(&dir, build(&calls));
        let run = e.run_sinks().await;
        assert!(
            run.ran().contains(&"mult"),
            "the corrupt cache is a same-run miss"
        );
        assert!(run.errored().is_empty(), "the recomputed run succeeds");
    }
    assert_eq!(calls.count(), 2);
    assert!(
        blob.exists(),
        "the corrupt blob is replaced by the same run"
    );

    // Reopen: mult's fresh blob is a clean hit → reused, not recomputed.
    {
        let mut e = disk_engine(&dir, build(&calls));
        let run = e.run_sinks().await;
        assert!(
            !run.ran().contains(&"mult"),
            "the replaced blob is a clean hit"
        );
    }
    assert_eq!(
        calls.count(),
        2,
        "the clean replacement prunes its producer"
    );
}

/// A persisted node whose disk blob is gone by the time the run reaches it
/// must recompute, not panic. A missing blob simply misses, so the node runs
/// and rewrites it — never pruned behind an absent value.
#[tokio::test]
async fn vanished_frontier_blob_recomputes_instead_of_panicking() {
    let dir = TempDir::new("vanish");
    let calls = Calls::default();

    // src → sum(Disk) → print. print reads sum, so sum is the frontier the
    // run must load.
    let build = |calls: &Calls| {
        let mut g = TestGraph::new();
        g.add("src", source(7, calls));
        g.add("sum", sum(CacheMode::Disk));
        g.add("print", |n| n.records());
        g.wire("src", 0, "sum", 0);
        g.wire("src", 0, "sum", 1);
        g.wire("sum", 0, "print", 0);
        g
    };

    let mut e = disk_engine(&dir, build(&calls));
    e.run_sinks().await;
    let after_run1 = calls.count();

    // Reopen, then remove sum's blob before the run reaches it.
    let mut e = disk_engine(&dir, build(&calls));
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let run = e.run_sinks().await;

    // The run completes — no panic: the missing blob just misses.
    assert!(
        run.ran().contains(&"sum"),
        "sum recomputes when its blob is gone"
    );
    assert!(
        !run.cached().contains(&"sum"),
        "a vanished blob is not served as a cache hit"
    );
    assert!(
        calls.count() > after_run1,
        "src re-ran to feed sum's recompute"
    );
}

/// A redefined output type can't serve a stale blob: `produce`'s func is
/// changed `Int → Float` with the same id, but the output signature is
/// folded into the content digest, so the Float node re-keys away from the
/// Int blob and recomputes — the consumer sees the correct `Float`, never
/// the stale `Int`.
#[tokio::test]
async fn redefined_output_type_rekeys_and_recomputes() {
    let dir = TempDir::new("wrong-type");
    let runs = Calls::default();
    let received = Arc::new(StdMutex::new(f64::NAN));

    // `produce` is a pure, Disk-persisted source whose declared output type
    // and value are `Int` or `Float`. Its id and inputs stay unchanged,
    // isolating output-signature invalidation.
    let build = |as_float: bool, runs: &Calls, received: &Arc<StdMutex<f64>>| {
        let received = received.clone();
        let (ty, value) = if as_float {
            (DataType::Float, StaticValue::Float(1.5))
        } else {
            (DataType::Int, StaticValue::Int(7))
        };
        let produce = runs.returning(value);
        let mut g = TestGraph::new();
        g.add("produce", move |n: NodeSpec| {
            n.pure().cache(CacheMode::Disk).output(ty).compute(produce)
        });
        g.add("consume", move |n: NodeSpec| {
            n.sink().input(DataType::Any).observes(move |inputs| {
                *received.lock().unwrap() = inputs[0].as_f64().unwrap_or(f64::NAN);
            })
        });
        g.wire("produce", 0, "consume", 0);
        g
    };

    // Run 1 (Int): produce runs and stores its Int blob; consume sees 7.
    let mut e = disk_engine(&dir, build(false, &runs, &received));
    e.run_sinks().await;
    assert_eq!(runs.count(), 1);
    assert_eq!(*received.lock().unwrap(), 7.0);

    // Run 2 (Float): the Float output re-keys produce's digest away from the
    // Int blob's key, so it isn't found — produce recomputes as Float.
    let mut e = disk_engine(&dir, build(true, &runs, &received));
    e.run_sinks().await;
    assert_eq!(
        runs.count(),
        2,
        "the Float output re-keys away from the stale Int blob, so produce recomputes"
    );
    assert_eq!(
        *received.lock().unwrap(),
        1.5,
        "consume receives the recomputed Float, never the stale Int"
    );
}
