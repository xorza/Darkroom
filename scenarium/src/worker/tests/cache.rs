use super::*;

/// Eviction is fire-and-forget: it happens inside the batch, before the
/// acknowledgement, and reports nothing when it succeeds.
#[tokio::test]
async fn a_successful_eviction_reports_nothing() {
    let mut w = TestWorker::over(TestGraph::sample());
    let compiled = w.compile();
    let get_a = w.id("get_a");

    w.settle([
        WorkerMessage::Update {
            compiled: Arc::clone(&compiled),
        },
        WorkerMessage::EvictCache { nodes: vec![get_a] },
    ])
    .await;

    let WorkerReport::Installed(installed) = w.report().await else {
        panic!("installation must be reported before cache eviction");
    };
    assert!(Arc::ptr_eq(&installed, &compiled));
    w.quiet();
}

#[tokio::test]
async fn an_eviction_failure_uses_the_general_worker_error_report() {
    let dir = TempDir::new("eviction-error");
    let mut w = TestWorker::over(TestGraph::sample());
    let blocked = w.id("get_a");
    // A directory where the blob file belongs: removal fails on it.
    let blocked_path = dir.join(blocked.as_uuid().simple().to_string());
    std::fs::create_dir(&blocked_path).unwrap();

    w.settle([
        w.disk_store(dir.path()),
        w.update(),
        WorkerMessage::EvictCache {
            nodes: vec![blocked],
        },
    ])
    .await;

    assert!(matches!(w.report().await, WorkerReport::Installed(_)));
    let WorkerReport::Error(WorkerError::CacheEviction {
        failure_count,
        details,
    }) = w.report().await
    else {
        panic!("a cache deletion failure must use the general worker error report");
    };
    assert_eq!(failure_count, 1);
    assert!(details.contains(&format!("{blocked:?}")));
    assert!(details.contains(&format!("failed to remove {}", blocked_path.display())));
    w.quiet();
}

/// `source` (pure, RAM) → `square` (pure, Disk) → `print` (sink).
fn disk_cached_graph(calls: &Calls) -> TestGraph {
    let mut graph = TestGraph::new();
    let source = calls.returning(7i64);
    graph.add("source", move |node| {
        node.pure()
            .cache(CacheMode::Ram)
            .output(DataType::Int)
            .compute(source)
    });
    graph.add("square", |node| {
        node.pure()
            .cache(CacheMode::Disk)
            .input(DataType::Int)
            .output(DataType::Int)
            .compute(|inputs| {
                let value = inputs[0].as_i64().unwrap();
                ConstValue::Int(value * value)
            })
    });
    graph.add("print", |node| node.records());
    graph.wire("source", 0, "square", 0);
    graph.wire("square", 0, "print", 0);
    graph
}

/// The disk cache wires through `SetDiskStore` and persists across worker
/// restarts: a `Disk` reproducible node's output, stored on a cold run,
/// reloads on a fresh worker over the same store so its upstream never
/// recomputes. `SetDiskStore` shares the batch with `Update`, proving it is
/// applied before the install hydrates.
#[tokio::test]
async fn a_disk_cached_node_survives_a_worker_restart() {
    let dir = TempDir::new("diskcache");
    let calls = Calls::default();

    let mut w = TestWorker::over(disk_cached_graph(&calls));
    w.send_many([w.disk_store(dir.path()), w.update(), TestWorker::sinks()]);
    let cold = w.run().await;
    assert_eq!(
        cold.ran(),
        ["print", "source", "square"],
        "a cold run computes every node"
    );
    assert_eq!(calls.count(), 1);
    assert_eq!(cold.logs(), ["49"]);

    // Reopen on a fresh worker over the same store: `square` loads from
    // disk and is reused. Its input `source` feeds only the reused
    // `square`, which never reads it, so the pre-run cut prunes it.
    let mut w = w.restart();
    w.send_many([w.disk_store(dir.path()), w.update(), TestWorker::sinks()]);
    let warm = w.run().await;
    assert_eq!(
        calls.count(),
        1,
        "the cut prunes the Ram input feeding only a disk-cache hit"
    );
    assert_eq!(warm.ran(), ["print"], "only the sink re-ran");
    assert_eq!(
        warm.cached(),
        ["square"],
        "square is served from the disk cache"
    );
    assert_eq!(warm.logs(), ["49"]);
}

/// `SetDiskStore` flushes resident disk-backed values into the just-attached
/// store: a `Both`-mode value computed while no store root existed (an
/// unsaved document) would otherwise be a RAM hit on every later run —
/// which never stores — and silently recompute on reopen.
#[tokio::test]
async fn attaching_a_store_flushes_resident_disk_backed_values() {
    let dir = TempDir::new("storeswap");
    let calls = Calls::default();
    let mut graph = disk_cached_graph(&calls);
    graph.cache("square", CacheMode::Both);

    // Run with no store root: the Both value stays resident-only.
    let mut w = TestWorker::over(graph);
    w.send_many([w.update(), TestWorker::sinks()]);
    w.run().await;
    assert_eq!(dir.entry_count(), 0);

    w.settle([w.disk_store(dir.path())]).await;

    assert_eq!(
        dir.entry_count(),
        1,
        "the resident Both-mode value was flushed into the new store"
    );
}
