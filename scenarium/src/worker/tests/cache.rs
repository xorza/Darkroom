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

    let WorkerReport::Installed {
        compiled: installed,
        ..
    } = w.report().await
    else {
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

    assert!(matches!(w.report().await, WorkerReport::Installed { .. }));
    let WorkerReport::Error(error) = w.report().await else {
        panic!("a cache deletion failure must use the general worker error report");
    };
    let WorkerError::CacheEviction { failures } = &error else {
        panic!("a deletion failure is an eviction error, got {error:?}");
    };
    let [failure] = failures.as_slice() else {
        panic!("only the blocked node fails, got {failures:?}");
    };
    assert_eq!(failure.node_id, blocked);
    let CacheNodeError::Removal(removal) = &failure.cause else {
        panic!(
            "a blocked deletion is a removal failure, got {:?}",
            failure.cause
        );
    };
    assert_eq!(removal.path, blocked_path);
    // The typed payload still renders as the line a host displays.
    assert!(
        error.to_string().contains(&format!("{blocked:?}")),
        "{error}"
    );
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

/// A flush that wrote nothing reports through the same general error the host
/// already shows for eviction, whether the store was just attached or the host
/// named the node. Silence used to be the only answer either way, which reads
/// exactly like success — while the node goes on showing itself disk-backed
/// with nothing behind it.
#[tokio::test]
async fn a_flush_failure_uses_the_general_worker_error_report() {
    let dir = TempDir::new("flush-error");
    let calls = Calls::default();
    let mut graph = disk_cached_graph(&calls);
    graph.cache("square", CacheMode::Both);

    // Compute with no store attached, leaving the value resident and unwritten.
    let mut w = TestWorker::over(graph);
    w.send_many([w.update(), TestWorker::sinks()]);
    w.run().await;

    // A directory where the blob file belongs: the body streams into the
    // temporary beside it, and the publication onto the destination fails.
    let blocked = w.id("square");
    let blocked_path = dir.join(blocked.as_uuid().simple().to_string());
    std::fs::create_dir(&blocked_path).unwrap();

    // The sweep the host asks for once the store is attached.
    w.settle([w.disk_store(dir.path()), WorkerMessage::FlushAllCaches])
        .await;
    let WorkerReport::Error(WorkerError::CacheFlush {
        failures,
        unsupported,
    }) = w.report().await
    else {
        panic!("a sweep that could not write must report it");
    };
    assert!(
        unsupported.is_empty(),
        "the type persists fine; the disk did not"
    );
    let [failure] = failures.as_slice() else {
        panic!("only `square` is disk-backed and resident, got {failures:?}");
    };
    assert_eq!(failure.node_id, blocked);
    let CacheNodeError::Store(StoreError::Publish { path, .. }) = &failure.cause else {
        panic!(
            "a blocked destination fails at publication, got {:?}",
            failure.cause
        );
    };
    assert_eq!(
        path, &blocked_path,
        "the failure names the blob it could not write"
    );
    w.quiet();

    // And again for the flush the host asks for by node — the `↓` chip's path.
    w.settle([WorkerMessage::FlushCache {
        nodes: vec![blocked],
    }])
    .await;
    let WorkerReport::Error(WorkerError::CacheFlush { failures, .. }) = w.report().await else {
        panic!("a requested flush that could not write must report it");
    };
    assert_eq!(failures.len(), 1);
    w.quiet();
}

/// A value whose type has no registered codec will never reach disk, however
/// often it is retried. A flush the host asked for by node reports that — there
/// the node is about to sit disk-backed with nothing behind it — while the
/// sweep a store attach performs over every node stays silent, since nobody
/// named those nodes and their types are a standing fact about the library.
#[tokio::test]
async fn an_unpersistable_type_is_reported_only_when_the_flush_was_requested() {
    use std::any::Any;
    use std::fmt;

    use crate::library::TypeEntry;
    use crate::{CustomValue, DynamicValue, TypeId};

    const BLOB_TYPE: &str = "9d17a6a2-8f97-4c74-a0b7-1b2c9a9f5f60";

    #[derive(Debug)]
    struct Blob;
    impl fmt::Display for Blob {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Blob")
        }
    }
    impl CustomValue for Blob {
        fn type_id(&self) -> TypeId {
            BLOB_TYPE.into()
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
            self
        }
    }

    let dir = TempDir::new("flush-unsupported");
    let mut graph = TestGraph::new();
    // Registered without a codec — and the store takes its codecs from this
    // same library, so the value is unwritable by construction.
    graph
        .library
        .register_type(BLOB_TYPE, TypeEntry::custom("Blob"));
    graph.add("make_blob", |node| {
        node.pure()
            .sink()
            .cache(CacheMode::Both)
            .output(DataType::Custom(BLOB_TYPE.into()))
            .lambda(async_lambda!(|Invocation { outputs, .. }| {
                outputs[0] = DynamicValue::Custom(Arc::new(Blob));
                Ok(())
            }))
    });

    // Compute with no store attached: the Both value is resident, unwritten.
    let mut w = TestWorker::over(graph);
    w.send_many([w.update(), TestWorker::sinks()]);
    w.run().await;

    w.settle([w.disk_store(dir.path()), WorkerMessage::FlushAllCaches])
        .await;
    w.quiet();
    assert_eq!(dir.entry_count(), 0, "nothing could be written");

    w.settle([WorkerMessage::FlushCache {
        nodes: vec![w.id("make_blob")],
    }])
    .await;
    let WorkerReport::Error(WorkerError::CacheFlush {
        failures,
        unsupported,
    }) = w.report().await
    else {
        panic!("a requested flush of an unpersistable value must report it");
    };
    assert!(
        failures.is_empty(),
        "nothing was attempted, so nothing failed"
    );
    let [skipped] = unsupported.as_slice() else {
        panic!("the one unpersistable node is the whole report, got {unsupported:?}");
    };
    assert_eq!(skipped.node_id, w.id("make_blob"));
    assert_eq!(
        skipped.type_id,
        TypeId::from(BLOB_TYPE),
        "the report names the type that cannot persist"
    );
    w.quiet();
}

/// An install re-measures the cache it reconciled onto, and the report carries
/// the figure: a program that dropped a node releases its resident value there
/// and then, while the next completed run may never correct the host — an empty
/// program does not execute at all. Without this the readout keeps naming a
/// cache that is already gone.
#[tokio::test]
async fn an_install_reports_the_cache_left_after_reconciling() {
    use std::any::Any;
    use std::fmt;

    use crate::library::TypeEntry;
    use crate::{CustomValue, DynamicValue, TypeId};

    const HEAVY_TYPE: &str = "c84cc23f-f313-4adb-8788-ef82c26691ae";
    const HEAVY_CPU: usize = 4096;

    #[derive(Debug)]
    struct Heavy;
    impl fmt::Display for Heavy {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Heavy")
        }
    }
    impl CustomValue for Heavy {
        fn type_id(&self) -> TypeId {
            HEAVY_TYPE.into()
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
            self
        }
        fn ram_bytes(&self) -> RamUsage {
            RamUsage {
                cpu: HEAVY_CPU,
                gpu: 0,
            }
        }
    }

    let mut graph = TestGraph::new();
    graph
        .library
        .register_type(HEAVY_TYPE, TypeEntry::custom("Heavy"));
    graph.add("heavy", |node| {
        node.pure()
            .sink()
            .cache(CacheMode::Ram)
            .output(DataType::Custom(HEAVY_TYPE.into()))
            .lambda(async_lambda!(|Invocation { outputs, .. }| {
                outputs[0] = DynamicValue::Custom(Arc::new(Heavy));
                Ok(())
            }))
    });

    // The first install lands on an empty cache; the run then fills it.
    let mut w = TestWorker::over(graph);
    w.send(w.update());
    let WorkerReport::Installed { cache_ram, .. } = w.report().await else {
        panic!("the fixture installs before it runs");
    };
    assert_eq!(
        cache_ram,
        RamUsage::default(),
        "a fresh worker reconciles onto nothing"
    );

    w.send(TestWorker::sinks());
    let run = w.run().await;
    assert_eq!(
        run.cache_ram,
        RamUsage {
            cpu: HEAVY_CPU,
            gpu: 0
        },
        "the one Ram-cached node's value is the whole resident set"
    );

    // Install a program without that node: reconcile drops its slot, and the
    // install says so — nothing else will, since an empty program never runs.
    w.graph = TestGraph::new();
    w.send(w.update());
    let WorkerReport::Installed { cache_ram, .. } = w.report().await else {
        panic!("the replacement program installs");
    };
    assert_eq!(
        cache_ram,
        RamUsage::default(),
        "the dropped node's value left with its slot"
    );

    w.settle([TestWorker::sinks()]).await;
    w.quiet();
}

/// `FlushAllCaches` writes resident disk-backed values into the attached
/// store: a `Both`-mode value computed while no store root existed (an
/// unsaved document) would otherwise be a RAM hit on every later run —
/// which never stores — and silently recompute on reopen.
///
/// And attaching the store *alone* writes nothing. The sweep used to be
/// inferred from the attach, which made every unrelated store swap — a document
/// opened, a library edit rebuilding the codec map — pay for a sweep, and
/// aimed some of them at the wrong root.
#[tokio::test]
async fn resident_disk_backed_values_are_flushed_when_asked_and_not_on_a_bare_attach() {
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
        0,
        "attaching a store is not a request to write into it"
    );

    w.settle([WorkerMessage::FlushAllCaches]).await;
    assert_eq!(
        dir.entry_count(),
        1,
        "the sweep wrote the resident Both-mode value into the attached store"
    );
}

/// The batch's graph op is the frame of reference for everything after it: a
/// batch that repoints the store *and* replaces the graph resolves the graph
/// first, so nothing of the outgoing program can be written into the incoming
/// one's root.
///
/// This is the document-open path. It used to write the closed document's
/// resident values into the opened document's cache directory, under ids
/// nothing there would ever read — and since no cache root is ever pruned,
/// those blobs were permanent.
#[tokio::test]
async fn a_batch_resolves_its_graph_before_the_store_it_repoints_to() {
    let outgoing = TempDir::new("outgoing-doc");
    let incoming = TempDir::new("incoming-doc");
    let calls = Calls::default();
    let mut graph = disk_cached_graph(&calls);
    graph.cache("square", CacheMode::Both);

    // The outgoing document: computed against its own store, value resident.
    let mut w = TestWorker::over(graph);
    w.send_many([
        w.disk_store(outgoing.path()),
        w.update(),
        TestWorker::sinks(),
    ]);
    w.run().await;
    assert_eq!(outgoing.entry_count(), 1, "it wrote its own blob");

    // The control, and what makes the assertion below mean anything: the same
    // repoint-and-sweep *without* a graph op does write, so the value really is
    // resident and really would follow the store to a new root.
    let elsewhere = TempDir::new("no-graph-op");
    w.settle([
        w.disk_store(elsewhere.path()),
        WorkerMessage::FlushAllCaches,
    ])
    .await;
    assert_eq!(
        elsewhere.entry_count(),
        1,
        "with the program still installed, the sweep follows the store"
    );

    // Closing it and opening another is one batch — drop the program, repoint
    // the store — and the host asks for the sweep too, which is the strongest
    // form of the case: even an explicit sweep has nothing left to write,
    // because the graph op ran first.
    w.settle([
        WorkerMessage::Clear,
        w.disk_store(incoming.path()),
        WorkerMessage::FlushAllCaches,
    ])
    .await;

    assert_eq!(
        incoming.entry_count(),
        0,
        "the closed document's values must not land in the opened document's store"
    );
    assert_eq!(
        outgoing.entry_count(),
        1,
        "and the store it did write stays as it was"
    );
}
