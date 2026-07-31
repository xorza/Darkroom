use std::sync::Arc;

use super::*;
use crate::execution::compile::error::CompileError;
use crate::execution::error::{Error, RunError};
use crate::graph::Binding;
use crate::graph::Graph;
use crate::graph::func::error::InvokeError;
use crate::graph::func::lambda::Invocation;
use crate::graph::func::lambda::OutputDemand;
use crate::graph::func::lambda::internals;
use crate::graph::func::{Func, FuncBehavior};
use crate::graph::identity::{InputPort, NodeId, OutputPort};
use crate::graph::node::{CacheMode, Node};
use crate::library::Library;
use crate::testing::engine::TestEngine;
use crate::testing::graph::{NodeSpec, TestGraph};
use crate::testing::{TestFuncHooks, test_func_lib, test_graph};
use crate::{DataType, DynamicValue, StaticValue};
use ::common::FloatExt;
use tokio::sync::Mutex;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn execution_node_name<'a>(
    execution_graph: &ExecutionEngine,
    graph: &'a Graph,
    _library: &'a Library,
    node_id: NodeId,
) -> &'a str {
    assert!(
        execution_graph.compiled().contains(node_id),
        "the node belongs to the installed program"
    );
    &graph.find(node_id).unwrap().name
}

fn execution_node_id(
    execution_graph: &ExecutionEngine,
    graph: &Graph,
    library: &Library,
    name: &str,
) -> Option<NodeId> {
    execution_graph
        .compiled()
        .node_ids
        .iter()
        .copied()
        .find(|&node_id| execution_node_name(execution_graph, graph, library, node_id) == name)
}

fn default_hooks() -> TestFuncHooks {
    TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    }
}

fn mutate_func(library: &mut Library, name: &str, mutate: impl FnOnce(&mut Func)) {
    let mut func = library.by_name(name).unwrap().clone();
    mutate(&mut func);
    library.remove(func.id).unwrap();
    library.add(func);
}

/// Instantiate a `Node` for `func_name` with a fixed id; caller wires bindings.
fn node(library: &Library, func_name: &str) -> Node {
    library.by_name(func_name).unwrap().into()
}

/// Set input `idx` of the named node's binding in the source graph.
fn bind(graph: &mut Graph, node_name: &str, idx: usize, binding: impl Into<Option<Binding>>) {
    let id = graph.find_by_name(node_name).unwrap().id;
    graph.set_input_binding(InputPort::new(id, idx), binding);
}

mod cache_persistence {
    use super::*;
    use crate::execution::cache::disk_store::DiskStore;
    use crate::execution::report::internals::CollectingReporter;
    use crate::execution::schedule::NodeState;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    /// A unique temp directory removed on drop, so tests don't collide or leak.
    #[derive(Debug)]
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "scenarium-engine-diskcache-{tag}-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fresh engine backed by a disk store rooted at `dir`
    /// (simulating a reopen when called twice against the same dir). The default
    /// empty library is fine — these tests cache plain values.
    fn disk_engine(dir: &TempDir) -> ExecutionEngine {
        use crate::library::Library;
        let mut engine = ExecutionEngine::default();
        engine
            .cache
            .set_disk_store(DiskStore::new(&Library::default(), Some(dir.0.clone())));
        engine
    }

    #[tokio::test]
    async fn explicit_cache_eviction_removes_the_downstream_ram_and_disk_cone() {
        let dir = TempDir::new("explicit-eviction");
        let get_a_calls = Arc::new(AtomicUsize::new(0));
        let get_b_calls = Arc::new(AtomicUsize::new(0));
        let printed = Arc::new(StdMutex::new(Vec::new()));
        let library = test_func_lib(TestFuncHooks {
            get_a: {
                let calls = get_a_calls.clone();
                Arc::new(move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(1)
                })
            },
            get_b: {
                let calls = get_b_calls.clone();
                Arc::new(move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    11
                })
            },
            print: {
                let values = printed.clone();
                Arc::new(move |value| values.lock().unwrap().push(value))
            },
        });
        let mut graph = test_graph();
        for name in ["get_a", "get_b", "sum", "mult"] {
            let node_id = graph.find_by_name(name).unwrap().id;
            graph.find_mut(node_id).unwrap().cache = CacheMode::Both;
        }
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let get_b_id = graph.find_by_name("get_b").unwrap().id;
        let expected_names = ["get_a", "sum", "mult", "Print"];

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &library).unwrap();
        engine.execute_sinks().await.unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 1);
        assert_eq!(get_b_calls.load(Ordering::SeqCst), 1);
        assert_eq!(*printed.lock().unwrap(), vec![132, 132]);
        assert_eq!(blob_count(&dir), 4);

        let failures = engine.evict_cache(&[get_a_id]).await;
        let mut expected: Vec<_> = expected_names
            .iter()
            .map(|name| execution_node_id(&engine, &graph, &library, name).unwrap())
            .collect();
        expected.sort_unstable();
        assert!(
            failures.is_empty(),
            "the selected source and its data consumers must all evict"
        );
        for node_id in &expected {
            assert!(
                engine.slot(*node_id).output_values().is_none(),
                "{node_id:?} must release its resident output"
            );
        }
        let get_b_eid = get_b_id;
        assert!(
            engine.slot(get_b_eid).output_values().is_some(),
            "an upstream sibling outside the consumer cone stays resident"
        );
        assert_eq!(blob_count(&dir), 1, "only get_b's disk blob remains");

        drop(engine);
        let mut reopened = disk_engine(&dir);
        reopened.update(&graph, &library).unwrap();
        let rerun = reopened.execute_sinks().await.unwrap();
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 2);
        assert_eq!(get_b_calls.load(Ordering::SeqCst), 1);
        assert_eq!(*printed.lock().unwrap(), vec![132, 132, 132]);
        let reran: HashSet<_> = rerun.ran_nodes().collect();
        for node_id in expected {
            assert!(
                reran.contains(&node_id),
                "every evicted node must recompute after reopening"
            );
        }
        assert!(
            !reran.contains(&get_b_eid),
            "the retained sibling blob must still be reusable after reopening"
        );

        let blocked_eid = get_a_id;
        let blocked_path = dir.0.join(blocked_eid.as_uuid().simple().to_string());
        std::fs::remove_file(&blocked_path).unwrap();
        std::fs::create_dir(&blocked_path).unwrap();

        let partial_failures = reopened.evict_cache(&[get_a_id]).await;
        let [failure] = partial_failures.as_slice() else {
            panic!("the undeletable get_a path must be the only eviction failure");
        };
        assert_eq!(failure.node_id, blocked_eid);
        assert!(
            failure
                .message
                .starts_with(&format!("failed to remove {}:", blocked_path.display()))
        );
        // The same cone as above, less the seed whose blob cannot be removed.
        let expected_successes: Vec<_> = ["sum", "mult", "Print"]
            .iter()
            .map(|name| execution_node_id(&reopened, &graph, &library, name).unwrap())
            .collect();
        assert!(
            reopened.slot(blocked_eid).output_values().is_some(),
            "a failed disk deletion must leave the matching RAM value resident"
        );
        for node_id in &expected_successes {
            assert!(
                reopened.slot(*node_id).output_values().is_none(),
                "{node_id:?} must still evict when another target fails"
            );
        }
    }

    /// A `persist` node's output survives a fresh engine (reopen), its sole-consumer
    /// upstream is pruned on the hit, and an input change invalidates it —
    /// *overwriting* the node's one blob rather than orphaning it beside a new one.
    #[tokio::test]
    async fn persist_output_survives_reopen_and_invalidates_on_digest_change() {
        let dir = TempDir::new("e2e");

        // `get_a` recompute counter, shared across every engine via the hook.
        let get_a_calls = Arc::new(AtomicUsize::new(0));
        let make_lib = || {
            let calls = get_a_calls.clone();
            test_func_lib(TestFuncHooks {
                get_a: Arc::new(move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(7)
                }),
                ..default_hooks()
            })
        };

        // get_a (pure source) → mult (pure, persist Disk) → print (sink).
        let lib = make_lib();
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a"));
        let mut mult = node(&lib, "mult");
        mult.cache = CacheMode::Disk;
        graph.add(mult);
        graph.add(node(&lib, "Print"));
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let mult_id = graph.find_by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        // First run: everything computes; `mult` is stored to disk.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 1);

        // Reopen: a fresh engine over the same store. `mult` loads from disk (reused). Its
        // only consumer of `get_a` is the reused `mult`, which never reads it, so the pre-run
        // cut prunes `get_a` — a `Memory` source with no cross-session cache is *not*
        // recomputed on reopen (the win the removed plan-time pass used to give).
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        let mut reporter = CollectingReporter::default();
        let mut stats = ExecutionOutcome::default();
        engine
            .execute(
                RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                &mut reporter,
                CancelToken::never(),
                &mut stats,
            )
            .await
            .unwrap();
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            1,
            "the cut prunes the Memory source upstream of a disk-cache hit on reopen"
        );
        assert!(!stats.ran(get_a_id), "get_a was cut, not executed");
        assert!(stats.cached(mult_id), "mult reused from disk");
        assert!(!stats.ran(mult_id), "mult did not recompute");
        assert!(
            !stats.node_ram(mult_id).total() > 0,
            "a full run does not retain the Disk node after the run"
        );
        let executed_allocation = stats.nodes.as_ptr();
        let executed_capacity = stats.nodes.capacity();
        assert!(executed_capacity > 0);

        // A targeted run on `mult` hydrates the disk hit, but targeting must not
        // turn it into an implicit RAM cache.
        let mut reporter = CollectingReporter::default();
        engine
            .execute(
                RunSeeds {
                    node_ids: vec![mult_id],
                    ..Default::default()
                },
                &mut reporter,
                CancelToken::never(),
                &mut stats,
            )
            .await
            .unwrap();
        assert!(stats.cached(mult_id));
        assert!(
            !stats.node_ram(mult_id).total() > 0,
            "a targeted run releases the hydrated Disk value"
        );
        assert_eq!(stats.nodes.as_ptr(), executed_allocation);
        assert_eq!(stats.nodes.capacity(), executed_capacity);

        // Changing one input to a const makes `mult` miss, while its other input
        // still needs `get_a`, so the cut keeps the source alive and it runs.
        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(3)));
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            2,
            "input change makes mult miss and recompute from get_a"
        );
        assert!(
            !stats.cached(mult_id),
            "mult should not be cached after a digest change"
        );
        // The blob is keyed by node id, so the recompute replaced the superseded
        // bytes in place — the old digest's cache doesn't linger as an orphan.
        assert_eq!(
            blob_count(&dir),
            1,
            "a digest change overwrites the node's blob, not adds a second"
        );
    }

    /// Fan-out: a producer feeding both a reuse hit *and* a running consumer must survive
    /// the cut — the running consumer still reads it. Proves the cut is a backward union
    /// over consumers, not a forward "all consumers reused" filter (which would wrongly
    /// prune the shared producer and starve the executing branch).
    #[tokio::test]
    async fn shared_producer_read_by_a_running_consumer_is_not_cut() {
        let dir = TempDir::new("fanout");

        let get_a_calls = Arc::new(AtomicUsize::new(0));
        let make_lib = || {
            let calls = get_a_calls.clone();
            test_func_lib(TestFuncHooks {
                get_a: Arc::new(move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(7)
                }),
                ..default_hooks()
            })
        };

        // get_a → mult(persist Disk) → print_mult ;  get_a → print_direct.
        let lib = make_lib();
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a"));
        let mut mult = node(&lib, "mult");
        mult.cache = CacheMode::Disk;
        graph.add(mult);
        let print_mult_id = NodeId::unique();
        graph.insert(print_mult_id, node(&lib, "Print"));
        let print_direct_id = NodeId::unique();
        graph.insert(print_direct_id, node(&lib, "Print"));
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let mult_id = graph.find_by_name("mult").unwrap().id;
        graph.set_input_binding(InputPort::new(mult_id, 0), Binding::bind(get_a_id, 0));
        graph.set_input_binding(InputPort::new(mult_id, 1), Binding::bind(get_a_id, 0));
        graph.set_input_binding(InputPort::new(print_mult_id, 0), Binding::bind(mult_id, 0));
        graph.set_input_binding(
            InputPort::new(print_direct_id, 0),
            Binding::bind(get_a_id, 0),
        );

        // Cold run: everything computes; mult is stored to disk.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 1);

        // Reopen: mult reuses from disk, so the get_a→mult edge is cut — but print_direct
        // still reads get_a, so the union keeps get_a alive and it recomputes.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            2,
            "get_a is still read by print_direct, so the cut must keep it"
        );
        assert!(
            stats.ran(get_a_id),
            "the shared producer runs for its executing consumer"
        );
        assert!(stats.cached(mult_id), "mult still reuses from disk");
    }

    /// Two disk-cached nodes chained (`sum` → `mult`) under an executing sink
    /// (`print`). On reopen only the frontier `mult` — the cached value `print`
    /// actually reads — is deserialized into RAM; the deeper `sum`, whose sole
    /// consumer `mult` is itself reused-from-disk, is never hydrated.
    #[tokio::test]
    async fn chained_disk_cache_hydrates_only_the_live_frontier() {
        let dir = TempDir::new("chain-frontier");

        let get_a_calls = Arc::new(AtomicUsize::new(0));
        let make_lib = || {
            let calls = get_a_calls.clone();
            test_func_lib(TestFuncHooks {
                get_a: Arc::new(move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(7)
                }),
                ..default_hooks()
            })
        };

        // get_a(7) → sum(Both) = 7+7 = 14 → mult(Both) = 14*7 = 98 → print. `Both`
        // (RAM + disk) so the frontier the run reads is kept resident — that retention
        // is what this test asserts (pure `Disk` would drop its RAM copy).
        let lib = make_lib();
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a"));
        let mut sum = node(&lib, "sum");
        sum.cache = CacheMode::Both;
        graph.add(sum);
        let mut mult = node(&lib, "mult");
        mult.cache = CacheMode::Both;
        graph.add(mult);
        graph.add(node(&lib, "Print"));
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let sum_id = graph.find_by_name("sum").unwrap().id;
        let mult_id = graph.find_by_name("mult").unwrap().id;
        bind(&mut graph, "sum", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "sum", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "mult", 0, Binding::bind(sum_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        // First run: everything computes; sum (14) and mult (98) stored to disk.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 1);

        // Reopen over the same store with fresh RAM. Resolution alone settles `mult` as a
        // reuse — its blob header covers the demand — without decoding the body: the value
        // only enters RAM when the run loop reaches the node, so a run's reusable frontier
        // never accumulates ahead of the first lambda.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        engine.prepare_execution(true, false, &[]).await.unwrap();
        assert_eq!(
            engine.node_state(mult_id),
            NodeState::Reuse,
            "the frontier blob is verified from its header during resolution"
        );
        assert!(
            engine.slot(mult_id).output_values().is_none(),
            "...and is not decoded there"
        );

        let stats = engine.execute_sinks().await.unwrap();

        // Only frontier `mult` is verified and reused. `sum` and `get_a` are behind that
        // hit, so neither is hydrated or recomputed.
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            1,
            "the cut prunes the Memory source feeding only disk-cache hits"
        );
        assert!(
            !stats.cached(sum_id) && stats.cached(mult_id),
            "only the live frontier cache is hydrated and reported"
        );

        // The frontier `mult` (read by the executing `print`) is in RAM...
        let mult_resident = engine.slot(mult_id).output_values().is_some();
        assert!(mult_resident, "frontier cache is loaded into RAM");
        // ...but the deeper `sum` is not even flagged: the blob stays in the store,
        // outside the runtime slot, until a later run actually needs it.
        let sum_resident = engine.slot(sum_id).output_values().is_some();
        let sum_empty = engine.slot(sum_id).output_values().is_none();
        assert!(
            !sum_resident,
            "an unneeded upstream disk cache is not hydrated"
        );
        assert!(
            sum_empty,
            "the deeper cache is not probed before exact demand reaches it"
        );

        let empty_dir = TempDir::new("chain-empty");
        engine.cache.set_disk_store(DiskStore::new(
            &Library::default(),
            Some(empty_dir.0.clone()),
        ));
        assert!(
            engine.slot(mult_id).output_values().is_some(),
            "switching stores preserves resident values"
        );

        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(3)));
        engine.update(&graph, &make_lib()).unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert!(
            stats.ran(sum_id),
            "a value absent from the new store recomputes when needed"
        );
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            2,
            "recomputing sum also restores its pruned memory-only input"
        );
    }

    /// A blob that satisfies the resolver's header probe but fails to decode when the run
    /// loop reaches it. The reuse verdict already cut the node's producers, so the run
    /// cannot fall back to recomputing: the node fails, its consumers skip as
    /// errored-upstream, and the undecodable blob is dropped so the next run recomputes.
    #[tokio::test]
    async fn a_probed_blob_that_stops_decoding_fails_its_node_and_self_heals() {
        let dir = TempDir::new("corrupt-frontier");

        let get_a_calls = Arc::new(AtomicUsize::new(0));
        let make_lib = || {
            let calls = get_a_calls.clone();
            test_func_lib(TestFuncHooks {
                get_a: Arc::new(move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(7)
                }),
                ..default_hooks()
            })
        };

        // get_a(7) → mult(Disk) = 49 → print.
        let lib = make_lib();
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a"));
        let mut mult = node(&lib, "mult");
        mult.cache = CacheMode::Disk;
        graph.add(mult);
        graph.add(node(&lib, "Print"));
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let mult_id = graph.find_by_name("mult").unwrap().id;
        let print_id = graph.find_by_name("Print").unwrap().id;
        bind(&mut graph, "mult", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 1);

        // Reopen, then corrupt the stored value while leaving the header that the probe
        // reads — digest, arity, and per-output coverage — untouched.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        engine.cache.disk_store().corrupt_payload(mult_id, 1);
        engine.prepare_execution(true, false, &[]).await.unwrap();
        assert_eq!(
            engine.node_state(mult_id),
            NodeState::Reuse,
            "a header-only probe cannot see a corrupt payload"
        );

        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            1,
            "the reuse verdict already pruned the producer, so nothing recomputes"
        );
        let error_for = |node_id| stats.error(node_id).cloned();
        assert!(
            matches!(error_for(mult_id), Some(RunError::CacheLoadFailed { .. })),
            "the node whose cache stopped loading fails, rather than serving nothing"
        );
        assert!(
            matches!(error_for(print_id), Some(RunError::SkippedUpstream { .. })),
            "its consumer skips as errored-upstream"
        );
        assert!(!stats.cached(mult_id));
        assert_eq!(
            blob_count(&dir),
            0,
            "the undecodable blob is dropped rather than left to fail every future run"
        );

        // Nothing left to reuse: the whole cone recomputes and republishes.
        let stats = engine.execute_sinks().await.unwrap();
        assert!(stats.errored_nodes().count() == 0, "the next run is clean");
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 2);
        assert_eq!(blob_count(&dir), 1);
    }

    /// A `Both` value remains resident even when a later run neither executes nor reads it.
    #[tokio::test]
    async fn both_value_stays_resident_outside_the_active_frontier() {
        let dir = TempDir::new("both-retained");
        let lib = test_func_lib(default_hooks());

        // get_a(1) → sum(Both) = 2 → mult(Both) = 2 → print.
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a"));
        let mut sum = node(&lib, "sum");
        sum.cache = CacheMode::Both;
        graph.add(sum);
        let mut mult = node(&lib, "mult");
        mult.cache = CacheMode::Both;
        graph.add(mult);
        graph.add(node(&lib, "Print"));
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let sum_id = graph.find_by_name("sum").unwrap().id;
        let mult_id = graph.find_by_name("mult").unwrap().id;
        bind(&mut graph, "sum", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "sum", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "mult", 0, Binding::bind(sum_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &lib).unwrap();

        engine.execute_sinks().await.unwrap();
        assert!(
            engine.slot(sum_id).output_values().is_some(),
            "sum is resident after the run that computed it"
        );

        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(stats.ran_node_count, 1, "only print runs the second time");

        let sum_slot = &engine.slot(sum_id);
        let mult_resident = engine.slot(mult_id).output_values().is_some();
        let get_a_resident = engine.slot(get_a_id).output_values().is_some();
        assert!(
            matches!(
                sum_slot.output_values().map(|values| &values[0]),
                Some(DynamicValue::Static(StaticValue::Int(2)))
            ),
            "Both keeps the exact prior-run value resident outside the active frontier"
        );
        assert!(
            mult_resident,
            "the frontier value the run read is kept resident"
        );
        assert!(
            get_a_resident,
            "a non-reloadable (Memory) value is kept, never force-recomputed"
        );

        // An empty replacement store proves the later hit comes from retained RAM, not disk.
        let empty_dir = TempDir::new("both-retained-empty");
        engine.cache.set_disk_store(DiskStore::new(
            &Library::default(),
            Some(empty_dir.0.clone()),
        ));
        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(3)));
        engine.update(&graph, &lib).unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert!(cached(&stats, sum_id), "sum is reused from retained RAM");
        assert!(!ran(&stats, sum_id), "sum does not recompute");
        assert!(
            ran(&stats, mult_id),
            "the changed downstream node recomputes"
        );
        assert!(
            engine.slot(sum_id).output_values().is_some(),
            "the reused Both value remains resident"
        );
    }

    /// A top-level node recomputed (rather than reused) in the last run.
    fn ran(stats: &ExecutionOutcome, id: NodeId) -> bool {
        stats.ran(id)
    }
    /// A top-level node reused a cache or remained resident behind a cut last run.
    fn cached(stats: &ExecutionOutcome, id: NodeId) -> bool {
        stats.cached(id)
    }
    /// Count of blobs in the store — one per persisted node.
    fn blob_count(dir: &TempDir) -> usize {
        std::fs::read_dir(&dir.0).unwrap().flatten().count()
    }

    /// One row of the cache-mode matrix. Over a fresh store, build `get_a → mult(mode) →
    /// print` (an impure sink, so `mult` is needed every run), run twice on one engine,
    /// then reopen with empty RAM. Asserts the four modes' *distinct* outcomes on the axes
    /// they differ on: cross-run reuse, RAM retention after the run, and disk persistence.
    async fn assert_mode_behavior(mode: CacheMode) {
        let dir = TempDir::new(&format!("mode-{mode:?}"));
        let lib = test_func_lib(default_hooks());

        // get_a(1) → mult(mode) = 1*1 = 1 → print.
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a"));
        let mut mult = node(&lib, "mult");
        mult.cache = mode;
        graph.add(mult);
        graph.add(node(&lib, "Print"));
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let mult_id = graph.find_by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        // Two runs on one engine: run 1 is cold; run 2 reveals cross-run reuse.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &lib).unwrap();
        let run1 = engine.execute_sinks().await.unwrap();
        assert!(
            ran(&run1, mult_id),
            "{mode:?}: mult computes on the cold run"
        );

        let run2 = engine.execute_sinks().await.unwrap();
        if mode == CacheMode::None {
            assert!(
                ran(&run2, mult_id),
                "None recomputes every run its value is needed"
            );
            assert!(!cached(&run2, mult_id), "None is never reported cached");
        } else {
            assert!(
                cached(&run2, mult_id),
                "{mode:?} reuses its cached output on run 2"
            );
            assert!(!ran(&run2, mult_id), "{mode:?} does not recompute on run 2");
        }

        // Slot retention after run 2: RAM-resident iff the mode keeps RAM.
        let slot = &engine.slot(mult_id);
        assert_eq!(
            slot.output_values().is_some(),
            mode.caches_in_ram(),
            "{mode:?}: RAM retention must equal caches_in_ram()"
        );
        match mode {
            CacheMode::None => assert!(
                slot.output_values().is_none(),
                "None drops its value after the run: {:?}",
                slot.output_values()
            ),
            CacheMode::Disk => assert!(
                slot.output_values().is_none(),
                "Disk drops its RAM copy after the run: {:?}",
                slot.output_values()
            ),
            CacheMode::Ram | CacheMode::Both => assert!(
                matches!(
                    slot.output_values().map(|v| &v[0]),
                    Some(DynamicValue::Static(StaticValue::Int(1)))
                ),
                "Ram/Both keep the resident value (1*1=1): {:?}",
                slot.output_values()
            ),
        }

        // A blob exists iff the mode persists to disk.
        assert_eq!(
            blob_count(&dir) > 0,
            mode.persists_to_disk(),
            "{mode:?}: a blob exists iff persists_to_disk()"
        );

        // Reopen with empty RAM over the same store: only a disk-backed mode survives.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &lib).unwrap();
        let reopen = engine.execute_sinks().await.unwrap();
        if mode.persists_to_disk() {
            assert!(
                cached(&reopen, mult_id),
                "{mode:?} reloads mult from disk on reopen"
            );
            assert!(
                !ran(&reopen, get_a_id),
                "{mode:?}: the cut prunes get_a behind the disk hit"
            );
        } else {
            assert!(
                ran(&reopen, mult_id),
                "{mode:?} has no disk blob, so mult recomputes on reopen"
            );
            assert!(
                ran(&reopen, get_a_id),
                "{mode:?}: get_a recomputes to feed mult"
            );
        }
    }

    /// The four cache modes produce four distinct reuse / retention / persistence
    /// behaviors — the parameterized proof that the mode actually drives the engine.
    #[tokio::test]
    async fn cache_mode_matrix() {
        for mode in [
            CacheMode::None,
            CacheMode::Ram,
            CacheMode::Disk,
            CacheMode::Both,
        ] {
            assert_mode_behavior(mode).await;
        }
    }

    /// `None` is storage-only: it never taints downstream reproducibility. `A(None) →
    /// B(Disk)` — B still has a content digest, so it persists and, on reopen, is served
    /// from disk with A cut (not recomputed), exactly as if A were an ordinary cached node.
    /// Contrast an `Impure` A, which *would* strip B of its digest and force both to rerun.
    #[tokio::test]
    async fn none_upstream_does_not_disable_downstream_disk_cache() {
        let dir = TempDir::new("none-orthogonal");
        let lib = test_func_lib(default_hooks());

        // get_a(1) → A = sum(None) = 1+1 = 2 → B = mult(Disk) = 2*2 = 4 → print.
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a"));
        let mut a = node(&lib, "sum");
        a.cache = CacheMode::None;
        graph.add(a);
        let mut b = node(&lib, "mult");
        b.cache = CacheMode::Disk;
        graph.add(b);
        graph.add(node(&lib, "Print"));
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let a_id = graph.find_by_name("sum").unwrap().id;
        let b_id = graph.find_by_name("mult").unwrap().id;
        bind(&mut graph, "sum", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "sum", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "mult", 0, Binding::bind(a_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(b_id, 0));

        // Cold run computes A and B; B(Disk) persists — proving A(None) left B a digest.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &lib).unwrap();
        let cold = engine.execute_sinks().await.unwrap();
        assert!(
            ran(&cold, a_id) && ran(&cold, b_id),
            "cold run computes A and B"
        );
        assert!(
            blob_count(&dir) > 0,
            "B(Disk) persists despite its None upstream"
        );

        // Reopen: B is a disk hit, so A(None) — read only by the reused B — is cut, not
        // recomputed. Setting A to None disabled neither B's cache nor A's own reuse-cut.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &lib).unwrap();
        let reopen = engine.execute_sinks().await.unwrap();
        assert!(cached(&reopen, b_id), "B reloads from disk on reopen");
        assert!(
            !ran(&reopen, a_id),
            "A(None) is cut behind the disk hit, not recomputed"
        );
        assert!(!ran(&reopen, get_a_id), "get_a is cut behind A too");
    }

    /// A valid disk blob for a node's *current* digest must be served even when the
    /// slot still holds a RAM value produced under a superseded digest — the stale
    /// resident value must not mask the fresh blob. Disk reuse must load the current
    /// blob before deciding that an older resident value is reusable.
    ///
    /// The intervening run uses `Ram` mode so it can't overwrite the node's one
    /// disk blob (a `Disk`-mode run would — the blob is keyed by node id).
    #[tokio::test(flavor = "multi_thread")]
    async fn stale_ram_value_does_not_mask_a_valid_disk_blob() -> TestResult {
        let dir = TempDir::new("flip_back");
        let printed = Arc::new(Mutex::new(Vec::new()));
        let printed_for_hook = printed.clone();
        let lib = test_func_lib(TestFuncHooks {
            print: Arc::new(move |value| {
                printed_for_hook.try_lock().unwrap().push(value);
            }),
            ..default_hooks()
        });

        // mult read by print. Const binds detach mult from any upstream, so its
        // digest is a pure function of the two consts. Fixed node ids so the slot
        // (and its resident value) survives each `update`.
        let build = |a: i64, b: i64, mode: CacheMode| {
            let mut graph = Graph::default();
            let mut mult = node(&lib, "mult");
            mult.cache = mode;
            graph.insert(NodeId::from_u128(1), mult);
            graph.insert(NodeId::from_u128(2), node(&lib, "Print"));
            let mult_id = graph.find_by_name("mult").unwrap().id;
            bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(a)));
            bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(b)));
            bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));
            graph
        };
        let mult_id = NodeId::from_u128(1);

        let mut engine = disk_engine(&dir);

        // Config A (Disk): mult = 2 * 3 = 6 → the blob (digest D_A) stored on disk.
        engine.update(&build(2, 3, CacheMode::Disk), &lib)?;
        engine.execute_sinks().await?;

        // Config B (Ram): mult = 5 * 7 = 35 → slot now resident with 35 under B's
        // digest; the disk blob still carries D_A (Ram mode never writes).
        engine.update(&build(5, 7, CacheMode::Ram), &lib)?;
        engine.execute_sinks().await?;

        // Flip back to A with Both so install preserves the current B snapshot in RAM.
        // Resolution then stamps A's digest, making 35 superseded before disk reuse probes
        // the matching A blob — it must serve 6 from disk, not the stale 35.
        engine.update(&build(2, 3, CacheMode::Both), &lib)?;
        assert_eq!(
            engine
                .slot(mult_id)
                .output_values()
                .and_then(|values| values[0].as_i64()),
            Some(35),
            "the stale B snapshot remains resident when the flip-back run begins"
        );
        let stats = engine.execute_sinks().await?;

        // mult is served from its disk blob, not recomputed — without this, a recompute
        // would yield 6 regardless and the stale-RAM path would go untested.
        assert!(
            !stats.ran(mult_id),
            "mult is a disk cache hit on flip-back, not recomputed: {:?}",
            stats.nodes
        );

        assert_eq!(
            printed.lock().await.as_slice(),
            &[6, 35, 6],
            "flip-back serves the disk blob (6), not the stale RAM value (35)"
        );
        Ok(())
    }

    /// A `persist` node is written to disk the moment *it* finishes, not in a batch at
    /// the end of the run — so its blob is already on disk by the time a downstream
    /// node executes. The sink `print` hook checks the store dir is non-empty when
    /// it runs; that holds only because `mult` was persisted right after it finished,
    /// before `print` started. (Batched-at-the-end storing would leave the dir empty
    /// here.)
    #[tokio::test(flavor = "multi_thread")]
    async fn persist_node_lands_on_disk_before_its_consumer_runs() {
        let dir = TempDir::new("per_node_store");
        let root = dir.0.clone();
        let blob_present_when_print_ran = Arc::new(AtomicBool::new(false));
        let flag = blob_present_when_print_ran.clone();
        let lib = test_func_lib(TestFuncHooks {
            print: Arc::new(move |_v| {
                let non_empty = std::fs::read_dir(&root)
                    .map(|mut entries| entries.next().is_some())
                    .unwrap_or(false);
                flag.store(non_empty, Ordering::SeqCst);
            }),
            ..default_hooks()
        });

        // mult(const 2, const 3) = 6, persist=Disk → print. Const binds detach mult
        // from any upstream, so only mult + print run.
        let mut graph = Graph::default();
        let mut mult = node(&lib, "mult");
        mult.cache = CacheMode::Disk;
        graph.add(mult);
        graph.add(node(&lib, "Print"));
        let mult_id = graph.find_by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(2)));
        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(3)));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &lib).unwrap();
        engine.execute_sinks().await.unwrap();

        assert!(
            blob_present_when_print_ran.load(Ordering::SeqCst),
            "mult's disk blob must exist when print runs (persisted per-node, not batched)"
        );
    }

    /// Disabling RAM retention releases a surviving slot during install rather than waiting
    /// for the end of a later run.
    #[tokio::test]
    async fn disabling_ram_retention_releases_resident_value_on_install() {
        let lib = test_func_lib(default_hooks());

        let build = |mode: CacheMode| {
            let mut graph = Graph::default();
            let mut mult = node(&lib, "mult");
            mult.cache = mode;
            graph.insert(NodeId::from_u128(1), mult);
            graph.insert(NodeId::from_u128(2), node(&lib, "Print"));
            let mult_id = graph.find_by_name("mult").unwrap().id;
            bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(2)));
            bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(3)));
            bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));
            graph
        };
        let mult_id = NodeId::from_u128(1);

        for mode in [CacheMode::None, CacheMode::Disk] {
            let dir = TempDir::new(&format!("ram-downgrade-{mode:?}"));
            let mut engine = disk_engine(&dir);
            engine.update(&build(CacheMode::Ram), &lib).unwrap();
            engine.execute_sinks().await.unwrap();
            assert!(
                engine.slot(mult_id).output_values().is_some(),
                "Ram retains the current pure value"
            );

            engine.update(&build(mode), &lib).unwrap();
            assert!(
                engine.slot(mult_id).output_values().is_none(),
                "{mode:?} releases the old RAM value during install"
            );
        }
    }

    /// `store_resident_caches` must not write a value under a digest it wasn't produced
    /// under. After an input change recompiles the program, a node's resident value is
    /// stale w.r.t. its new digest; flushing it stamped with D_B would overwrite the
    /// node's blob with bytes a later run at D_B would load as a false hit.
    #[tokio::test]
    async fn flush_skips_a_value_stale_for_the_current_digest() {
        let dir = TempDir::new("stale_flush");
        let lib = test_func_lib(default_hooks());

        // mult(persist=Disk) with const inputs → print; the consts drive mult's digest.
        let build = |a: i64, b: i64| {
            let mut graph = Graph::default();
            let mut mult = node(&lib, "mult");
            mult.cache = CacheMode::Disk;
            graph.insert(NodeId::from_u128(1), mult);
            graph.insert(NodeId::from_u128(2), node(&lib, "Print"));
            let mult_id = graph.find_by_name("mult").unwrap().id;
            bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(a)));
            bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(b)));
            bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));
            graph
        };

        let mut engine = disk_engine(&dir);

        // Config A: mult runs and is stored, stamped with its digest D_A (one blob).
        engine.update(&build(2, 3), &lib).unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(blob_count(&dir), 1, "config A's blob is stored");
        let blob_path = std::fs::read_dir(&dir.0)
            .unwrap()
            .flatten()
            .next()
            .unwrap()
            .path();
        let blob_a = std::fs::read(&blob_path).unwrap();

        // Config B: mult's inputs change ⇒ its *current* digest is now D_B, but the
        // resident value (6) was produced under D_A. Recompile (update), no re-run, then
        // flush — the stale value must not be re-stamped D_B (the blob is keyed by node
        // id, so a bad flush would show as an overwrite, not a second file).
        engine.update(&build(5, 7), &lib).unwrap();
        engine.store_resident_caches().await;
        assert_eq!(
            std::fs::read(&blob_path).unwrap(),
            blob_a,
            "a value stale for the current digest is not flushed (blob untouched)"
        );
    }

    /// A corrupt / incompatible cache blob must be *deleted* on a failed load so the
    /// same run recomputes and writes a fresh one. Without the delete, `store_node`'s
    /// skip-if-exists keeps the broken file and the node recomputes on *every* run
    /// (the regression: an old-format blob rejected by the outer format version was never
    /// replaced). Each "session" is a fresh engine, so the disk cache is the only source.
    #[tokio::test]
    async fn corrupt_blob_recomputes_and_is_replaced_in_the_same_run() {
        let dir = TempDir::new("corrupt_replace");
        let get_a_calls = Arc::new(AtomicUsize::new(0));
        let lib = {
            let calls = get_a_calls.clone();
            test_func_lib(TestFuncHooks {
                get_a: Arc::new(move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(1)
                }),
                print: Arc::new(|_| {}),
                ..default_hooks()
            })
        };

        // get_a → mult(persist=Disk) → print. mult reads get_a, so a mult cache hit
        // prunes get_a — its call count tracks whether mult actually recomputed.
        let mut graph = Graph::default();
        graph.insert(NodeId::from_u128(1), node(&lib, "get_a"));
        let mut mult = node(&lib, "mult");
        mult.cache = CacheMode::Disk;
        graph.insert(NodeId::from_u128(2), mult);
        graph.insert(NodeId::from_u128(3), node(&lib, "Print"));
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let mult_id = graph.find_by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        let mult_id = graph.find_by_name("mult").unwrap().id;
        let ran = |s: &ExecutionOutcome, id: NodeId| s.ran(id);

        // Cold run: mult computes and stores its blob.
        {
            let mut engine = disk_engine(&dir);
            engine.update(&graph, &lib).unwrap();
            let stats = engine.execute_sinks().await.unwrap();
            assert!(ran(&stats, mult_id), "cold run computes mult");
        }
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 1);

        // Corrupt mult's blob *body* (a torn write / an old, version-mismatched format)
        // while keeping the leading 32-byte digest header intact — a garbled header
        // would already fail the presence probe and never reach body verification
        // this test is about.
        let blob = std::fs::read_dir(&dir.0)
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

        // Reopen: the corrupt blob still carries the current digest in its header. Body
        // verification fails before the resolver cuts the producer cone, so the blob is
        // deleted and mult recomputes successfully in this same run.
        {
            let mut engine = disk_engine(&dir);
            engine.update(&graph, &lib).unwrap();
            let stats = engine.execute_sinks().await.unwrap();
            assert!(ran(&stats, mult_id), "the corrupt cache is a same-run miss");
            assert!(
                stats.errored_nodes().count() == 0,
                "the recomputed run succeeds"
            );
        }
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 2);
        assert!(
            blob.exists(),
            "the corrupt blob is replaced by the same run"
        );

        // Reopen: mult's fresh blob is a clean hit → reused, not recomputed.
        {
            let mut engine = disk_engine(&dir);
            engine.update(&graph, &lib).unwrap();
            let stats = engine.execute_sinks().await.unwrap();
            assert!(!ran(&stats, mult_id), "the replaced blob is a clean hit");
        }
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            2,
            "the clean replacement prunes its producer"
        );
    }

    /// A `persist` node whose disk blob is gone by the time the run reaches it must
    /// recompute, not panic. A missing blob simply misses, so the node runs and rewrites
    /// it — never pruned behind an absent value.
    #[tokio::test]
    async fn vanished_frontier_blob_recomputes_instead_of_panicking() {
        let dir = TempDir::new("vanish");
        let recompute = Arc::new(AtomicUsize::new(0));
        let make_lib = || {
            let calls = recompute.clone();
            test_func_lib(TestFuncHooks {
                get_a: Arc::new(move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(7)
                }),
                ..default_hooks()
            })
        };

        // get_a → sum(persist) → print(sink). print reads sum, so sum is the
        // frontier the run must load.
        let lib = make_lib();
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a"));
        let mut sum = node(&lib, "sum");
        sum.cache = CacheMode::Disk;
        graph.add(sum);
        graph.add(node(&lib, "Print"));
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let sum_id = graph.find_by_name("sum").unwrap().id;
        bind(&mut graph, "sum", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "sum", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(sum_id, 0));

        // Run 1: writes sum's blob to disk.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        engine.execute_sinks().await.unwrap();
        let after_run1 = recompute.load(Ordering::SeqCst);

        // Reopen, then remove sum's blob before the run reaches it.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        for entry in std::fs::read_dir(&dir.0).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        let stats = engine.execute_sinks().await.unwrap();

        // The run completes (no panic): the missing blob just misses, so sum recomputes.
        assert!(stats.ran(sum_id), "sum recomputes when its blob is gone");
        assert!(
            !stats.cached(sum_id),
            "a vanished blob is not served as a cache hit"
        );
        assert!(
            recompute.load(Ordering::SeqCst) > after_run1,
            "get_a re-ran to feed sum's recompute"
        );
    }

    /// A redefined output type can't serve a stale blob: `produce`'s func is changed
    /// `Int → Float` with the same id, but the output signature is folded into the
    /// content digest, so the Float node re-keys away from the Int blob and recomputes
    /// — the consumer sees the correct `Float`, never the stale `Int`.
    #[tokio::test]
    async fn redefined_output_type_rekeys_and_recomputes() {
        use std::sync::Mutex;

        use crate::async_lambda;
        use crate::graph::func::{Func, FuncInput, FuncOutput};
        use crate::library::Library;

        const PRODUCE: &str = "63b7a83c-d7fc-46f4-805a-4bf2695e3763";
        const CONSUME: &str = "39bbd6b3-b919-4095-b3d0-79a4515de75e";

        let dir = TempDir::new("wrong-type");
        let produce_runs = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(f64::NAN));

        // `produce` is a pure, Disk-persisted source; its declared output type and
        // value are `Int` when `as_float` is false, `Float` when true. The func id and
        // inputs stay unchanged, isolating output-signature invalidation. `consume`
        // (sink) reads it and records the value as f64.
        let build_lib = |as_float: bool| -> Library {
            let mut lib = Library::default();
            let produce = Func::new(PRODUCE, "produce")
                .category("Test")
                .pure()
                .output(FuncOutput::new(
                    "out",
                    if as_float {
                        DataType::Float
                    } else {
                        DataType::Int
                    },
                ));
            let runs = produce_runs.clone();
            let produce = if as_float {
                produce.lambda(
                    async_lambda!(move |Invocation { outputs, .. }| { runs = runs.clone() } => {
                        runs.fetch_add(1, Ordering::SeqCst);
                        outputs[0] = DynamicValue::Static(StaticValue::Float(1.5));
                        Ok(())
                    }),
                )
            } else {
                produce.lambda(
                    async_lambda!(move |Invocation { outputs, .. }| { runs = runs.clone() } => {
                        runs.fetch_add(1, Ordering::SeqCst);
                        outputs[0] = DynamicValue::Static(StaticValue::Int(7));
                        Ok(())
                    }),
                )
            };
            lib.add(produce);
            let recv = received.clone();
            lib.add(
                Func::new(CONSUME, "consume")
                    .category("Test")
                    .sink()
                    .input(FuncInput::required("in", DataType::Any))
                    .lambda(
                        async_lambda!(move |Invocation { inputs, .. }| { recv = recv.clone() } => {
                            *recv.lock().unwrap() = inputs[0].as_f64().unwrap_or(f64::NAN);
                            Ok(())
                        }),
                    ),
            );
            lib
        };

        let engine_with = |lib: Library| {
            let mut eg = ExecutionEngine::default();
            eg.cache
                .set_disk_store(DiskStore::new(&lib, Some(dir.0.clone())));
            eg
        };

        // produce(persist) → consume(sink).
        let int_lib = build_lib(false);
        let mut graph = Graph::default();
        let mut produce_node = node(&int_lib, "produce");
        produce_node.cache = CacheMode::Disk;
        let produce_id = graph.add(produce_node);
        graph.add(node(&int_lib, "consume"));
        bind(&mut graph, "consume", 0, Binding::bind(produce_id, 0));

        // Run 1 (Int): produce runs, stores its Int blob; consume sees 7.
        let mut engine = engine_with(build_lib(false));
        engine.update(&graph, &int_lib).unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(produce_runs.load(Ordering::SeqCst), 1);
        assert_eq!(*received.lock().unwrap(), 7.0);

        // Run 2 (Float): the Float output re-keys produce's digest away from the Int
        // blob's key, so it isn't found — produce recomputes as Float.
        let float_lib = build_lib(true);
        let mut engine = engine_with(build_lib(true));
        engine.update(&graph, &float_lib).unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(
            produce_runs.load(Ordering::SeqCst),
            2,
            "the Float output re-keys away from the stale Int blob, so produce recomputes"
        );
        assert_eq!(
            *received.lock().unwrap(),
            1.5,
            "consume receives the recomputed Float, never the stale Int"
        );
    }

    /// A `persist` node whose cone contains an impure node has digest `None`, so
    /// it's never disk-cached even with `persist=Disk` — on reopen it recomputes.
    #[tokio::test]
    async fn impure_cone_persist_node_is_not_disk_cached() {
        let dir = TempDir::new("impure-cone");
        let mut library = test_func_lib(default_hooks());
        mutate_func(&mut library, "get_b", |func| {
            func.behavior = FuncBehavior::Impure;
        });

        // get_b (impure) → mult (persist) → print. mult's cone is impure.
        let mut graph = Graph::default();
        graph.add(node(&library, "get_b"));
        let mut mult = node(&library, "mult");
        mult.cache = CacheMode::Disk;
        graph.add(mult);
        graph.add(node(&library, "Print"));
        let get_b_id = graph.find_by_name("get_b").unwrap().id;
        let mult_id = graph.find_by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, Binding::bind(get_b_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(get_b_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &library).unwrap();
        engine.execute_sinks().await.unwrap();

        // Reopen: mult must recompute — an impure cone has no digest, so it never caches to disk.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &library).unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert!(
            !stats.cached(mult_id),
            "impure-cone node must not be disk-cached"
        );
        assert!(stats.ran(mult_id), "mult recomputes on reopen");
    }

    /// A `persist = Memory` node (the default) is never written to disk even though
    /// its cone is reproducible — only `Disk` opts in — so on reopen it recomputes.
    #[tokio::test]
    async fn memory_persistence_node_is_not_disk_cached() {
        let dir = TempDir::new("memory-persist");
        let library = test_func_lib(default_hooks());

        // get_a (pure) → mult (Memory, the default) → print.
        let mut graph = Graph::default();
        graph.add(node(&library, "get_a"));
        graph.add(node(&library, "mult"));
        graph.add(node(&library, "Print"));
        let get_a_id = graph.find_by_name("get_a").unwrap().id;
        let mult_id = graph.find_by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &library).unwrap();
        engine.execute_sinks().await.unwrap();

        // Reopen: fresh RAM, nothing on disk for mult ⇒ it recomputes.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &library).unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert!(
            !stats.cached(mult_id),
            "a Memory-persistence node must not be disk-cached"
        );
        assert!(stats.ran(mult_id), "mult recomputes on reopen");
    }

    /// A `persist` node whose blob is on disk but whose custom output type has *no
    /// registered codec* (a value written by a build that had the codec, reopened by one
    /// that doesn't) is not reused from disk. It recomputes rather than panicking during
    /// loading; with the codec it is served from disk instead.
    #[tokio::test]
    async fn missing_codec_skips_disk_cache_instead_of_panicking() {
        use std::any::Any;
        use std::fmt;

        use async_trait::async_trait;
        use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

        use crate::async_lambda;
        use crate::data::codec::error::CodecError;
        use crate::graph::func::{Func, FuncOutput};
        use crate::library::{Library, TypeEntry};
        use crate::runtime::context::{ContextStore, ContextType};
        use crate::{CustomValue, CustomValueCodec, TypeId};

        /// Decode-side context resource: proves a codec can reach the runtime
        /// store while reconstructing a value read from disk.
        #[derive(Debug, Default)]
        struct DecodeProbe {
            decodes: usize,
        }
        const DECODE_PROBE: ContextType<DecodeProbe> = ContextType::new(DecodeProbe::default);

        const BLOB_TYPE: &str = "50be7976-6d55-4567-8389-13107b1698ba";
        const FUNC_ID: &str = "b1ddc0bf-5f92-4e0c-9481-23e48c65004b";

        #[derive(Debug)]
        struct Blob(Vec<u8>);
        impl fmt::Display for Blob {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "Blob({} bytes)", self.0.len())
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
        #[derive(Debug)]
        struct BlobCodec;
        #[async_trait]
        impl CustomValueCodec for BlobCodec {
            fn version(&self) -> u32 {
                0
            }

            async fn encode(
                &self,
                value: &dyn CustomValue,
                writer: &mut (dyn AsyncWrite + Unpin + Send),
                _ctx: &mut ContextStore,
            ) -> std::result::Result<(), CodecError> {
                writer
                    .write_all(&value.as_any().downcast_ref::<Blob>().unwrap().0)
                    .await?;
                Ok(())
            }
            async fn decode(
                &self,
                reader: &mut (dyn AsyncRead + Unpin + Send),
                byte_len: u64,
                ctx: &mut ContextStore,
            ) -> std::result::Result<Arc<dyn CustomValue>, CodecError> {
                ctx.get(DECODE_PROBE).decodes += 1;
                let mut bytes = Vec::with_capacity(usize::try_from(byte_len)?);
                reader.read_to_end(&mut bytes).await?;
                Ok(Arc::new(Blob(bytes)))
            }
        }

        // A pure, sink, disk-persisted func emitting a custom `Blob`. The type's
        // codec is present only when `with_codec`.
        let blob_lib = |with_codec: bool, recompute: Arc<AtomicUsize>| -> Library {
            let mut library = Library::default();
            library.register_type(
                BLOB_TYPE,
                if with_codec {
                    TypeEntry::custom_with_codec("Blob", Arc::new(BlobCodec))
                } else {
                    TypeEntry::custom("Blob")
                },
            );
            library.add(
                Func::new(FUNC_ID, "make_blob")
                    .category("Test")
                    .pure()
                    .sink()
                    .output(FuncOutput::new("out", DataType::Custom(BLOB_TYPE.into())))
                    .lambda(async_lambda!(
                        move |Invocation { outputs, .. }| { counter = recompute.clone() } => {
                            counter.fetch_add(1, Ordering::SeqCst);
                            outputs[0] = DynamicValue::Custom(Arc::new(Blob(vec![9, 9, 9])));
                            Ok(())
                        }
                    )),
            );
            library
        };

        let disk_engine_with_lib = |dir: &TempDir, library: Library| {
            let mut engine = ExecutionEngine::default();
            engine
                .cache
                .set_disk_store(DiskStore::new(&library, Some(dir.0.clone())));
            engine
        };

        let dir = TempDir::new("missing-codec");
        let recompute = Arc::new(AtomicUsize::new(0));

        let mut graph = Graph::default();
        let mut blob_node = node(&blob_lib(true, recompute.clone()), "make_blob");
        blob_node.cache = CacheMode::Disk;
        let blob_id = graph.add(blob_node);

        // Run 1 (codec present): computes + writes the Blob to disk.
        let mut engine = disk_engine_with_lib(&dir, blob_lib(true, recompute.clone()));
        engine
            .update(&graph, &blob_lib(true, recompute.clone()))
            .unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(recompute.load(Ordering::SeqCst), 1, "cold run computes");

        // Reopen with codec: served from disk (no recompute); inspection decodes it.
        let mut engine = disk_engine_with_lib(&dir, blob_lib(true, recompute.clone()));
        engine
            .update(&graph, &blob_lib(true, recompute.clone()))
            .unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            recompute.load(Ordering::SeqCst),
            1,
            "codec present ⇒ served"
        );
        assert!(stats.cached(blob_id), "blob node disk-cached");
        assert_eq!(
            engine
                .executor
                .ctx_manager
                .contexts
                .get(DECODE_PROBE)
                .decodes,
            1,
            "the hydration decode reached the engine's runtime context store"
        );

        // Reopen WITHOUT codec: blob present but undecodable ⇒ not flagged available
        // ⇒ recompute, no panic.
        let mut engine = disk_engine_with_lib(&dir, blob_lib(false, recompute.clone()));
        engine
            .update(&graph, &blob_lib(false, recompute.clone()))
            .unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            recompute.load(Ordering::SeqCst),
            2,
            "missing codec ⇒ recompute"
        );
        assert!(
            !stats.cached(blob_id),
            "an undecodable blob is not a cache hit"
        );
        assert!(
            stats.ran(blob_id),
            "the node recomputes instead of tripping a failed frontier load"
        );
    }
}

mod resource_binds {
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use super::*;
    use crate::async_lambda;
    use crate::graph::func::{Func, FuncInput, FuncOutput};
    use crate::{FsPathConfig, FsPathMode};

    const MAKE_PATH: &str = "be2c3976-3a4f-4ed3-bfe6-8eafb35f084a";
    const LOAD_TEXT: &str = "5abcd2e7-f023-4122-8215-f6305c8b4a7e";
    const ANNOTATE: &str = "b8d6cc90-3c6e-4bdc-aaed-30b6740a9d5d";
    const CAPTURE: &str = "1a9629a9-dfbe-4665-b2b9-6f0d5c21f290";

    /// A temp file path removed on drop.
    #[derive(Debug)]
    struct TempFile(PathBuf);
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn temp_file(tag: &str) -> TempFile {
        static C: AtomicU64 = AtomicU64::new(0);
        let n = C.fetch_add(1, Ordering::Relaxed);
        TempFile(std::env::temp_dir().join(format!(
            "scenarium-resbind-{tag}-{}-{n}.bin",
            std::process::id()
        )))
    }

    /// A unique temp directory removed on drop (the disk store root for the reopen test).
    #[derive(Debug)]
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            static C: AtomicU64 = AtomicU64::new(0);
            let n = C.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "scenarium-resbind-{tag}-{}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn disk_engine(dir: &TempDir) -> ExecutionEngine {
        use crate::execution::cache::disk_store::DiskStore;
        let mut engine = ExecutionEngine::default();
        engine
            .cache
            .set_disk_store(DiskStore::new(&Library::default(), Some(dir.0.clone())));
        engine
    }

    /// The sink both fixtures share: records the received value's text.
    fn capture_func(captured: Arc<StdMutex<String>>) -> Func {
        Func::new(CAPTURE, "capture")
            .category("Test")
            .sink()
            .input(FuncInput::required("Value", DataType::Any))
            .lambda(async_lambda!(
                move |Invocation { inputs, .. }| { captured = captured.clone() } => {
                    *captured.lock().unwrap() =
                        inputs[0].as_string().unwrap_or_default().to_string();
                    Ok(())
                }
            ))
    }

    /// `make_path` (pure: `String` const in → `FsPath` value out — a producer whose own
    /// digest does *not* track the file, like any path-computing node) → `load_text`
    /// (pure: declared-`FsPath` input, reads the file, counts invocations) → `annotate`
    /// (pure, *downstream* of the late-stamped loader: brackets the text, counts
    /// invocations — proves the reach-time re-stamp cascades so downstream caches still
    /// hit) → `capture`.
    fn path_lib(
        loads: Arc<AtomicUsize>,
        annotates: Arc<AtomicUsize>,
        captured: Arc<StdMutex<String>>,
    ) -> Library {
        let mut lib = Library::default();
        lib.add(
            Func::new(MAKE_PATH, "make_path")
                .category("Test")
                .pure()
                .input(FuncInput::required("Name", DataType::String))
                .output(FuncOutput::new(
                    "Path",
                    DataType::FsPath(Arc::new(FsPathConfig::default())),
                ))
                .lambda(async_lambda!(
                    move |Invocation {
                              inputs, outputs, ..
                          }| {
                        let path = inputs[0].as_string().unwrap().to_string();
                        outputs[0] = StaticValue::FsPath(path).into();
                        Ok(())
                    }
                )),
        );
        lib.add(
            Func::new(LOAD_TEXT, "load_text")
                .category("Test")
                .pure()
                .input(FuncInput::required(
                    "Path",
                    DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFile))),
                ))
                .output(FuncOutput::new("Text", DataType::String))
                .lambda(async_lambda!(
                    move |Invocation { inputs, outputs, .. }| { loads = loads.clone() } => {
                        loads.fetch_add(1, Ordering::SeqCst);
                        let path = inputs[0].as_fs_path().unwrap().to_string();
                        let text =
                            std::fs::read_to_string(&path).map_err(InvokeError::external)?;
                        outputs[0] = StaticValue::String(text).into();
                        Ok(())
                    }
                )),
        );
        lib.add(
            Func::new(ANNOTATE, "annotate")
                .category("Test")
                .pure()
                .input(FuncInput::required("Text", DataType::String))
                .output(FuncOutput::new("Text", DataType::String))
                .lambda(async_lambda!(
                    move |Invocation { inputs, outputs, .. }| { annotates = annotates.clone() } => {
                        annotates.fetch_add(1, Ordering::SeqCst);
                        let text = inputs[0].as_string().unwrap();
                        outputs[0] = StaticValue::String(format!("[{text}]")).into();
                        Ok(())
                    }
                )),
        );
        lib.add(capture_func(captured));
        lib
    }

    #[derive(Debug)]
    struct PathFixture {
        graph: Graph,
        make_id: NodeId,
        load_id: NodeId,
        annotate_id: NodeId,
    }

    /// `make_path(const name = data path) → load_text → annotate → capture`, the three
    /// pure nodes on the given cache mode. Fixed node ids so reopened engines address the
    /// same slots.
    fn path_graph(lib: &Library, data_path: &str, mode: CacheMode) -> PathFixture {
        let mut graph = Graph::default();
        let mut make = node(lib, "make_path");
        make.cache = mode;
        graph.insert(NodeId::from_u128(1), make);
        let mut load = node(lib, "load_text");
        load.cache = mode;
        graph.insert(NodeId::from_u128(2), load);
        let mut annotate = node(lib, "annotate");
        annotate.cache = mode;
        graph.insert(NodeId::from_u128(4), annotate);
        graph.insert(NodeId::from_u128(3), node(lib, "capture"));
        let make_id = graph.find_by_name("make_path").unwrap().id;
        let load_id = graph.find_by_name("load_text").unwrap().id;
        let annotate_id = graph.find_by_name("annotate").unwrap().id;
        bind(
            &mut graph,
            "make_path",
            0,
            Binding::Const(StaticValue::String(data_path.to_string())),
        );
        bind(&mut graph, "load_text", 0, Binding::bind(make_id, 0));
        bind(&mut graph, "annotate", 0, Binding::bind(load_id, 0));
        bind(&mut graph, "capture", 0, Binding::bind(annotate_id, 0));
        PathFixture {
            graph,
            make_id,
            load_id,
            annotate_id,
        }
    }

    fn ran(stats: &ExecutionOutcome, id: NodeId) -> bool {
        stats.ran(id)
    }

    /// The core regression: a path arriving over a **Bind** edge keys the loader on the
    /// file behind the *delivered value*. Editing the file re-keys and recomputes the
    /// loader (pre-fix the chain's digests never changed, so the stale decode was served
    /// forever), while an unchanged file still reuses the cache — the reach-time re-stamp
    /// A declared path with no determinate identity fails **the node that
    /// declares it**, and its dependents skip as errored-upstream.
    ///
    /// Leaving it unstamped is correct — the node recomputes rather than
    /// reusing a result keyed on a guess — but it is also silent: the only
    /// symptom is a cache that stops hitting, with nothing said. Failing
    /// the whole run is the other extreme, and the pre-run sweep cannot
    /// even name a culprit, since it batches every executing node's paths
    /// at once. Both routes therefore converge on the node: a const path
    /// the sweep could not stamp leaves its digest `None`, which sends it
    /// through the same per-node stamp a producer-supplied path takes.
    #[tokio::test]
    #[cfg(unix)]
    async fn an_unidentifiable_path_fails_only_the_node_declaring_it() {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new("locked");
        let data = dir.0.join("data.txt");
        std::fs::write(&data, "v1").unwrap();
        let data_path = data.to_string_lossy().into_owned();
        let lock = |mode| std::fs::set_permissions(&dir.0, Permissions::from_mode(mode)).unwrap();
        let path_library = || {
            path_lib(
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(StdMutex::new(String::new())),
            )
        };
        // `loader` failed for want of a path identity, `dependent` was
        // skipped for reading it, and the run itself still succeeded.
        let assert_unavailable = |run: Result<ExecutionOutcome>, loader, dependent| {
            let stats = run.expect("a per-node failure must not abort the run");
            let error_for = |node_id| stats.error(node_id).cloned();
            assert!(
                matches!(
                    error_for(loader),
                    Some(RunError::ResourceUnavailable { .. })
                ),
                "the node declaring the path must fail: {:?}",
                error_for(loader),
            );
            assert!(
                matches!(error_for(dependent), Some(RunError::SkippedUpstream { .. })),
                "its dependent must skip as errored-upstream: {:?}",
                error_for(dependent),
            );
        };

        // A const path, known before the run: the sweep cannot stamp it,
        // so the node reaches its turn with no digest and re-stamps there.
        let lib = path_library();
        let mut graph = Graph::default();
        let load_id = NodeId::from_u128(2);
        graph.insert(load_id, node(&lib, "load_text"));
        let capture_id = NodeId::from_u128(4);
        graph.insert(capture_id, node(&lib, "capture"));
        graph.set_input_binding(
            InputPort::new(load_id, 0),
            Binding::Const(StaticValue::FsPath(data_path.clone())),
        );
        graph.set_input_binding(InputPort::new(capture_id, 0), Binding::bind(load_id, 0));

        let mut engine = ExecutionEngine::default();
        engine.update(&graph, &lib).unwrap();
        lock(0o000);
        let run = engine.execute_sinks().await;
        lock(0o755);
        assert_unavailable(run, load_id, capture_id);

        // The same file reached through a producer's value, known only at
        // the node's turn. Same outcome, same route.
        let lib = path_library();
        let fx = path_graph(&lib, &data_path, CacheMode::None);
        let mut engine = ExecutionEngine::default();
        engine.update(&fx.graph, &lib).unwrap();
        lock(0o000);
        let run = engine.execute_sinks().await;
        lock(0o755);
        assert_unavailable(run, fx.load_id, fx.annotate_id);
    }

    /// keeps wired-path loaders cacheable instead of tainting them uncacheable.
    #[tokio::test]
    async fn wired_path_rekeys_loader_on_file_change() {
        let data = temp_file("ram");
        std::fs::write(&data.0, "v1").unwrap();
        let loads = Arc::new(AtomicUsize::new(0));
        let annotates = Arc::new(AtomicUsize::new(0));
        let captured = Arc::new(StdMutex::new(String::new()));
        let lib = path_lib(loads.clone(), annotates.clone(), captured.clone());
        let fx = path_graph(&lib, &data.0.to_string_lossy(), CacheMode::Ram);

        let mut engine = ExecutionEngine::default();
        engine.update(&fx.graph, &lib).unwrap();

        // Cold run: everything computes (the loader's pre-run digest is None — the
        // delivered value doesn't exist yet — so it re-stamps at reach time and runs).
        engine.execute_sinks().await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(annotates.load(Ordering::SeqCst), 1);
        assert_eq!(*captured.lock().unwrap(), "[v1]");

        // Unchanged file: the loader reuses its RAM value under the full digest (producer
        // port + live file identity), and its *downstream* — whose digest folds the
        // loader's — skips too.
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "unchanged file ⇒ the wired-path loader stays cached"
        );
        assert!(stats.cached(fx.load_id));
        assert_eq!(
            annotates.load(Ordering::SeqCst),
            1,
            "downstream of the late-stamped loader skips compute on its hit"
        );
        assert!(stats.cached(fx.annotate_id));

        // Edit the file (different length ⇒ unambiguous identity change). The loader
        // re-keys off the delivered value's file identity and the change propagates to its
        // downstream — while the structural upstream (make_path) stays a RAM hit.
        std::fs::write(&data.0, "v2-longer").unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            2,
            "a file edit re-keys the loader through the wired path"
        );
        assert_eq!(
            annotates.load(Ordering::SeqCst),
            2,
            "the loader's new digest invalidates its downstream"
        );
        assert_eq!(
            *captured.lock().unwrap(),
            "[v2-longer]",
            "the fresh content flows downstream"
        );
        assert!(
            !ran(&stats, fx.make_id),
            "the path producer itself stays cached — nothing structural changed"
        );
        assert!(ran(&stats, fx.load_id));
    }

    /// Disk persistence across a reopen with a wired path: the loader's blob is keyed
    /// under the delivered path's live identity, so a fresh engine reuses it while the
    /// file is unchanged — hydrating the on-disk path producer just to stamp — and
    /// recomputes once the file changes, while the producer itself stays a disk hit. The
    /// downstream `annotate` proves the re-stamp *cascade*: on reopen its own pre-run
    /// digest is `None` too (it folds the loader's), and its reach-time re-stamp lands on
    /// its blob — the whole tainted cone skips compute, not just the loader.
    #[tokio::test]
    async fn wired_path_disk_reuse_survives_reopen_until_file_changes() {
        let dir = TempDir::new("disk");
        let data = temp_file("disk-data");
        std::fs::write(&data.0, "v1").unwrap();
        let loads = Arc::new(AtomicUsize::new(0));
        let annotates = Arc::new(AtomicUsize::new(0));
        let captured = Arc::new(StdMutex::new(String::new()));
        let lib = path_lib(loads.clone(), annotates.clone(), captured.clone());
        let fx = path_graph(&lib, &data.0.to_string_lossy(), CacheMode::Disk);

        // Cold run: computes and stores the blobs.
        let mut engine = disk_engine(&dir);
        engine.update(&fx.graph, &lib).unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(annotates.load(Ordering::SeqCst), 1);

        // Reopen, unchanged file: the loader is a disk hit under the re-stamped digest,
        // and so is its downstream — each re-stamped at reach time, producer-first.
        let mut engine = disk_engine(&dir);
        engine.update(&fx.graph, &lib).unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "reopen with an unchanged file serves the loader from disk"
        );
        assert!(stats.cached(fx.load_id));
        assert!(!ran(&stats, fx.load_id));
        assert_eq!(
            annotates.load(Ordering::SeqCst),
            1,
            "downstream of the late-stamped loader is a disk hit too"
        );
        assert!(stats.cached(fx.annotate_id));
        assert_eq!(
            *captured.lock().unwrap(),
            "[v1]",
            "the sink reads the hydrated disk value"
        );

        // Reopen after an edit: the loader's key moved ⇒ recompute, propagating to its
        // downstream; the path producer's own digest is unchanged, so it stays a disk hit
        // feeding the recompute.
        std::fs::write(&data.0, "v2-longer").unwrap();
        let mut engine = disk_engine(&dir);
        engine.update(&fx.graph, &lib).unwrap();
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            loads.load(Ordering::SeqCst),
            2,
            "reopen after a file edit recomputes the loader"
        );
        assert_eq!(
            annotates.load(Ordering::SeqCst),
            2,
            "the loader's new digest invalidates its downstream"
        );
        assert_eq!(*captured.lock().unwrap(), "[v2-longer]");
        assert!(
            !ran(&stats, fx.make_id),
            "the path producer is served from its blob, not recomputed"
        );
    }
}

mod graph_structure {
    use super::*;

    #[tokio::test]
    async fn basic_run() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());

        let plan = e.plan_sinks().await?;

        assert_eq!(plan.scheduled(), ["get_b", "get_a", "sum", "mult", "Print"]);
        assert_eq!(
            plan.runnable(),
            ["Print", "get_a", "get_b", "mult", "sum"],
            "an unedited fixture blocks nothing"
        );
        assert!(plan.missing_inputs().is_empty());

        // get_a→sum[0], get_b→sum[1]+mult[1], sum→mult[0], mult→print[0].
        for name in ["get_a", "get_b", "sum", "mult"] {
            assert_eq!(e.demand(name), [OutputDemand::Produce], "{name} demand");
        }
        assert_eq!(e.readers("get_a"), [1]);
        assert_eq!(e.readers("get_b"), [2], "feeds both sum[1] and mult[1]");
        assert_eq!(e.readers("sum"), [1]);
        assert_eq!(e.readers("mult"), [1]);

        assert!(e.engine.compiled().by_id(e.id("Print")).sink);
        Ok(())
    }

    #[tokio::test]
    async fn updates_after_graph_change() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        // Rewire mult to the sources directly, bypassing sum.
        e.edit(|g| {
            g.wire("get_a", 0, "mult", 0);
            g.wire("get_b", 0, "mult", 1);
        });

        let plan = e.plan_sinks().await?;

        assert_eq!(
            plan.scheduled(),
            ["get_b", "get_a", "mult", "Print"],
            "sum is no longer in any sink's cone"
        );
        for name in ["get_a", "get_b", "mult"] {
            assert_eq!(e.demand(name), [OutputDemand::Produce], "{name} demand");
            assert_eq!(e.readers(name), [1], "{name} now has exactly one consumer");
        }
        assert!(e.demand("Print").is_empty());
        Ok(())
    }

    #[test]
    fn update_rejects_func_missing_from_lib_and_keeps_prior_program() {
        let mut e = TestEngine::over(TestGraph::sample());
        assert_eq!(e.engine.compiled().e_nodes.len(), 5);

        // Recompiling the same graph against a library that defines none of its
        // funcs is rejected with a message naming a missing func.
        e.graph.library = Library::default();
        let CompileError { message } = e.try_reinstall().unwrap_err();
        assert!(
            message.contains("absent from the library"),
            "message should explain the missing func, got: {message}"
        );

        // The rejection happens before any mutation, so the prior program is
        // left intact rather than torn down.
        assert_eq!(e.engine.compiled().e_nodes.len(), 5);
    }
}

mod missing_inputs {
    use super::*;

    /// A required input left unbound blocks its node, and the verdict travels
    /// down every consumer — so the whole tail of the chain is out of the run
    /// while its sources still stand.
    #[tokio::test]
    async fn required_missing_propagates_downstream() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.edit(|g| g.unbind("sum", 0));

        let plan = e.plan_sinks().await?;

        assert_eq!(plan.missing_inputs(), ["Print", "mult", "sum"]);
        assert_eq!(
            plan.runnable(),
            ["get_b"],
            "the one source still feeding something stands; unbinding sum[0] left \
             `get_a` reading into nothing, so the backward walk never reaches it"
        );
        Ok(())
    }

    /// A *binding* to a missing-required producer propagates even through an
    /// **optional** input: the wired value can't be delivered, so the consumer
    /// (and its consumers) are missing too. Optionality only excuses an
    /// *unbound* input (see `optional_unbound_does_not_propagate`), not a
    /// binding to a broken upstream.
    #[tokio::test]
    async fn optional_bind_to_missing_propagates() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.edit(|g| {
            // sum missing-required; mult[0] stays bound to sum but goes optional.
            g.unbind("sum", 0);
            g.edit_func("mult", |func| func.inputs[0].required = false);
        });

        let plan = e.plan_sinks().await?;

        assert_eq!(plan.missing_inputs(), ["Print", "mult", "sum"]);
        assert_eq!(plan.runnable(), ["get_b"]);
        Ok(())
    }

    /// The contrast to `optional_bind_to_missing_propagates`: an optional input
    /// left **unbound** is a deliberate no-value, so it does not flag the node
    /// missing — it runs with its default.
    #[tokio::test]
    async fn optional_unbound_does_not_propagate() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.edit(|g| {
            g.unbind("mult", 0);
            g.edit_func("mult", |func| func.inputs[0].required = false);
        });

        let plan = e.plan_sinks().await?;

        assert!(plan.missing_inputs().is_empty());
        assert!(plan.runnable().contains(&"mult"));
        Ok(())
    }

    /// Executing counterpart: an optional bind to a gated upstream gates the
    /// consumer chain, so the executor never reads the absent output. Regression
    /// for the worker panicking in `collect_inputs` ("missing output values") —
    /// the planned-only siblings above can't catch it since they never execute.
    #[tokio::test(flavor = "multi_thread")]
    async fn optional_bind_to_gated_upstream_is_gated() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.edit(|g| {
            // sum's required input[0] unbound → sum missing-required → gated.
            g.unbind("sum", 0);
            // mult[0] (required) gets a real value; mult[1] is the only bind to
            // the gated sum and is *optional*, so this exercises optional-bind
            // propagation specifically. mult and print end up gated.
            g.wire("get_b", 0, "mult", 0);
            g.wire("sum", 0, "mult", 1);
            g.edit_func("mult", |func| func.inputs[1].required = false);
        });

        // Pre-fix, this panicked the worker; now the chain is gated and nothing runs.
        let run = e.run_sinks().await;

        assert_eq!(run.missing_inputs(), ["Print", "mult", "sum"]);
        assert_eq!(
            run.ran(),
            [] as [&str; 0],
            "the gated chain never runs, so it never reads sum's absent output — \
             and `get_b`, whose only consumer is gated, is cut with it"
        );
        Ok(())
    }
}

mod disabled_nodes {
    use super::*;
    use crate::execution::schedule::NodeState;

    /// Disabling `sum` retains it in the compiled program but excludes it from
    /// the plan. Its consumer `mult` sees the disabled producer as unavailable,
    /// so the missing-required-input flag propagates downstream.
    #[tokio::test]
    async fn disabled_node_stays_compiled_but_breaks_downstream() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.edit(|g| g.disable("sum"));

        let plan = e.plan_sinks().await?;

        assert!(
            e.engine.compiled().by_id(e.id("sum")).disabled,
            "the compiled node retains its authored disabled state"
        );
        assert_eq!(
            plan.state("sum"),
            NodeState::Disabled,
            "an unseeded disabled node stays structural but outside execution order"
        );
        assert_eq!(
            plan.missing_inputs(),
            ["Print", "mult"],
            "the consumers lost their transitive producer"
        );
        Ok(())
    }

    /// With `mult`'s sum-fed input made optional, disabling `sum` no longer
    /// breaks the chain: `sum` is skipped but `get_b → mult → print` still
    /// runs (mirrors `optional_unbound_does_not_propagate`, but via the
    /// disable flag rather than a cleared binding).
    #[tokio::test]
    async fn disabled_upstream_with_optional_consumer_still_runs() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.edit(|g| {
            g.disable("sum");
            g.edit_func("mult", |func| func.inputs[0].required = false);
        });

        let plan = e.plan_sinks().await?;

        assert_eq!(plan.scheduled(), ["get_b", "mult", "Print"]);
        Ok(())
    }

    /// …and the same shape must survive **execution**, not just planning.
    ///
    /// The consumer is schedulable and the disabled producer is not in
    /// `process_order` — but resolution marked every bound producer live
    /// regardless, so it registered a reader for an output nothing would
    /// ever write. Collecting the consumer's inputs then demanded that
    /// output: on a cold cache the run died on `a resolved producer
    /// output must be resident when consumed`, and on a warm one it
    /// silently served whatever the producer had left in RAM from before
    /// it was disabled, as if it were this run's value.
    ///
    /// `mult`'s `B` is the optional port, so the disabled producer feeds
    /// *that* one; unbound is what optional means, and its lambda's
    /// `unwrap_or(1)` is what reads it.
    #[tokio::test]
    async fn a_disabled_producer_on_an_optional_input_delivers_unbound() -> TestResult {
        let mut g = TestGraph::new();
        g.add("src", |n| n.returns(7i64));
        g.add("disabled", |n| {
            n.pure()
                .input(DataType::Int)
                .output(DataType::Int)
                .compute(|inputs| inputs[0].as_i64().unwrap_or_default().into())
        });
        // `b` is the optional port, so the disabled producer feeds *that* one;
        // unbound is what optional means, and the `unwrap_or(1)` below is what
        // reads it.
        g.add("mult", |n| {
            n.pure()
                .input(DataType::Int)
                .optional(DataType::Int)
                .output(DataType::Int)
                .compute(|inputs| {
                    let a = inputs[0].as_i64().expect("the required input is fed");
                    let b = inputs[1].as_i64().unwrap_or(1);
                    (a * b).into()
                })
        });
        g.add("print", |n| n.records());
        g.wire("src", 0, "disabled", 0);
        g.wire("src", 0, "mult", 0);
        g.wire("disabled", 0, "mult", 1);
        g.wire("mult", 0, "print", 0);

        let mut e = TestEngine::over(g);
        e.edit(|g| g.disable("disabled"));

        let run = e.run_sinks().await;

        assert_eq!(
            run.ran(),
            ["src", "mult", "print"],
            "the disabled producer stays out of the run",
        );
        assert_eq!(
            run.logs(),
            ["7"],
            "the optional input read as unbound, so `mult` multiplied by its \
             own default of 1 rather than reading a value nothing wrote",
        );
        Ok(())
    }
}

mod const_bindings {
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
}

mod behavior {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test(flavor = "multi_thread")]
    async fn pure_node_skips_on_rerun() -> TestResult {
        // `get_b` is a pure source, so once its output is cached its digest is
        // unchanged on a re-run and it reuses that value rather than running.
        let mut e = TestEngine::over(TestGraph::sample());

        assert!(e.run_sinks().await.ran().contains(&"get_b"));
        assert!(!e.run_sinks().await.ran().contains(&"get_b"));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn default_node_skips_on_rerun() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());

        let first = e.run_sinks().await;
        assert_eq!(first.ran(), ["get_b", "get_a", "sum", "mult", "Print"]);

        // Second run: only print (an impure sink) re-executes.
        let second = e.run_sinks().await;
        assert_eq!(second.ran(), ["Print"]);
        assert_eq!(second.cached(), ["get_a", "get_b", "mult", "sum"]);

        // The cached mult must still hold the correct product, not a stale
        // value: sum = get_a(1) + get_b(11) = 12; mult = 12 * get_b(11) = 132.
        assert_eq!(e.output_i64("mult", 0), Some(132));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn execute_emits_started_then_finished_progress_per_node() -> TestResult {
        use crate::execution::report::RunPhase;

        let mut e = TestEngine::over(TestGraph::sample());
        let (run, progress) = e.run_sinks_reporting().await;

        // Events come in Started→Finished pairs for the *same* node: the
        // executor is sequential, so each node brackets before the next starts.
        assert_eq!(progress.len() % 2, 0, "paired events");
        let mut started: Vec<&str> = Vec::new();
        for pair in progress.chunks_exact(2) {
            let (started_name, started_phase) = &pair[0];
            let (finished_name, finished_phase) = &pair[1];
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
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn execute_honors_cancel_flag_and_marks_cancelled() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());

        // Pre-tripped: the executor breaks at the first loop-top check, so no
        // node runs and the run is flagged cancelled.
        let tripped = CancelToken::new();
        tripped.cancel();
        let run = e.run_cancellable(RunSeeds::sinks(), tripped).await?;
        assert!(run.cancelled, "pre-tripped run is cancelled");
        assert_eq!(run.ran_node_count, 0, "no node runs when cancel is set");

        // A fresh token runs the whole graph — nothing was cached by the run
        // that aborted above.
        let run = e
            .run_cancellable(RunSeeds::sinks(), CancelToken::new())
            .await?;
        assert!(!run.cancelled);
        assert_eq!(run.ran_node_count, 5, "all nodes run when not cancelled");
        Ok(())
    }

    /// A node cancelled *mid-invoke* (the run is cancelled while its lambda
    /// runs) must not be reported executed and must not cache its partial
    /// output — otherwise the next run treats it as already computed. Models
    /// "start a run, immediately cancel it": the in-flight node bails with `Ok`
    /// but its result is bogus.
    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_mid_invoke_drops_in_flight_node_and_reruns() -> TestResult {
        use crate::async_lambda;

        // Trips the cancel on its first invoke only, so the re-run completes.
        let cancel_first = Arc::new(AtomicBool::new(true));
        let mut g = TestGraph::new();
        g.add("self_cancel", |n| {
            let cancel_first = cancel_first.clone();
            n.pure().sink().output(DataType::Int).lambda(async_lambda!(
                move |Invocation { ctx, outputs, .. }| { cancel_first = cancel_first.clone() } => {
                    if cancel_first.swap(false, Ordering::Relaxed) {
                        // Stand in for the user hitting Cancel while this runs.
                        ctx.cancel_flag().cancel();
                    }
                    outputs[0] = StaticValue::Int(7).into();
                    Ok(())
                }
            ))
        });
        let mut e = TestEngine::over(g);

        let run = e
            .run_cancellable(RunSeeds::sinks(), CancelToken::new())
            .await?;
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
            .await?;
        assert!(!run.cancelled);
        assert_eq!(
            run.ran_node_count, 1,
            "it re-runs; its output was not cached"
        );
        assert!(
            run.cached().is_empty(),
            "a cancelled node must not be served from cache on the next run"
        );
        Ok(())
    }

    /// A lambda that bails by returning `InvokeError::Cancelled` is reported as
    /// `RunError::Cancelled` (not a generic `Invoke` error) and dropped from the
    /// executed set — the truthful lambda-level signal, distinct from the
    /// executor's flag-check fallback covered above (asserted here without
    /// touching the flag, so only the error mapping can produce the verdict).
    #[tokio::test(flavor = "multi_thread")]
    async fn lambda_cancelled_error_maps_to_error_cancelled() -> TestResult {
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
        Ok(())
    }

    #[tokio::test]
    async fn impure_node_always_invoked() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample_with(TestFuncHooks::default()));
        e.edit(|g| g.edit_func("get_b", |func| func.behavior = FuncBehavior::Impure));

        // Even holding a cached output, an impure node still wants to execute.
        e.set_output("get_b", vec![StaticValue::Int(7).into()]);
        let plan = e.plan_sinks().await?;

        assert_eq!(plan.scheduled(), ["get_b", "get_a", "sum", "mult", "Print"]);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn impure_output_is_released_after_run() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.edit(|g| g.edit_func("get_b", |func| func.behavior = FuncBehavior::Impure));

        e.run_sinks().await;

        assert!(
            !e.holds_output("get_b"),
            "an impure value cannot hit on a future run, so the end sweep releases it"
        );
        Ok(())
    }
}

mod cycle_detection {
    use super::*;

    #[tokio::test]
    async fn returns_error_with_node_id() {
        let mut e = TestEngine::over(TestGraph::sample());
        // Close the loop: sum[0] ← mult, and mult already depends on sum.
        e.edit(|g| g.wire("mult", 0, "sum", 0));

        let error = e
            .plan_sinks()
            .await
            .expect_err("a cyclic graph cannot be planned");

        assert!(
            matches!(error, Error::CycleDetected { node_id } if node_id == e.id("mult")),
            "unexpected error: {error:?}"
        );
    }
}

mod installation {
    use super::*;
    use crate::testing::program::ProgramBuilder;

    /// A program of `ids`, nothing but identities — enough for the pairing the
    /// engine establishes at install.
    fn program(ids: &[NodeId]) -> Arc<CompiledGraph> {
        let mut prog = ProgramBuilder::default();
        for &node_id in ids {
            prog.node().id(node_id).add();
        }
        Arc::new(prog.into_program())
    }

    #[test]
    fn install_holds_one_canonical_artifact_for_the_engine_and_its_cache() {
        let compiled = program(&[NodeId::from_u128(1)]);
        let mut engine = ExecutionEngine::default();

        engine.install(Arc::clone(&compiled));

        assert!(Arc::ptr_eq(engine.compiled.as_ref().unwrap(), &compiled));
        engine.validate().unwrap();
    }

    /// A reinstall re-pairs the slots by stable id against the program being
    /// left, which the engine supplies — so a node that survives the recompile
    /// keeps its slot even when its dense index moves.
    #[test]
    fn install_carries_slots_across_a_shifted_index_space() {
        let surviving = NodeId::from_u128(2);

        let mut engine = ExecutionEngine::default();
        engine.install(program(&[NodeId::from_u128(1), surviving]));
        // Index 1 before the recompile: ids place in ascending order.
        engine.cache[NodeIdx(1)].state.set(17_u32);

        // Node 1 is dropped, so the survivor slides to index 0.
        engine.install(program(&[surviving, NodeId::from_u128(3)]));

        assert_eq!(engine.cache[NodeIdx(0)].state.get::<u32>(), Some(&17));
        assert!(engine.cache[NodeIdx(1)].state.is_none());
        engine.validate().unwrap();
    }

    #[test]
    fn validation_rejects_a_cache_with_the_wrong_node_count() {
        let engine = ExecutionEngine {
            compiled: Some(program(&[NodeId::from_u128(1)])),
            ..Default::default()
        };

        assert_eq!(
            engine.validate().unwrap_err().to_string(),
            "runtime cache spans 0 nodes, not the compiled program's 1"
        );
    }
}

mod invalidation {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn clear_resets_graph() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.run_sinks().await;
        assert!(!e.engine.compiled().e_nodes.is_empty());

        e.engine.clear();

        assert!(e.engine.compiled.is_none());
        assert!(e.engine.schedule.process_order.is_empty());
        assert_eq!(e.engine.cache.slot_count(), 0);
        Ok(())
    }
}

mod execution {
    use super::*;
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

    /// A pure source whose value a test can move under it — the point being
    /// that a *pure* node's digest does not notice, so the cached value stands
    /// until something re-keys it.
    fn shifting_source(cell: Arc<AtomicI64>) -> impl FnOnce(NodeSpec) -> NodeSpec {
        move |n: NodeSpec| {
            n.pure()
                .cache(CacheMode::Ram)
                .output(DataType::Int)
                .compute(move |_| cell.load(Ordering::Relaxed).into())
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn simple_compute() -> TestResult {
        let b = Arc::new(AtomicI64::new(5));
        let mut g = TestGraph::new();
        g.add("a", shifting_source(Arc::new(AtomicI64::new(2))));
        g.add("b", shifting_source(b.clone()));
        g.add("sum", |n| {
            n.pure()
                .cache(CacheMode::Ram)
                .input(DataType::Int)
                .input(DataType::Int)
                .output(DataType::Int)
                .compute(|i| (i[0].as_i64().unwrap() + i[1].as_i64().unwrap()).into())
        });
        g.add("mult", |n| {
            n.pure()
                .cache(CacheMode::Ram)
                .input(DataType::Int)
                .input(DataType::Int)
                .output(DataType::Int)
                .compute(|i| (i[0].as_i64().unwrap() * i[1].as_i64().unwrap()).into())
        });
        g.add("print", |n| n.records());
        g.wire("a", 0, "sum", 0);
        g.wire("b", 0, "sum", 1);
        g.wire("sum", 0, "mult", 0);
        g.wire("b", 0, "mult", 1);
        g.wire("mult", 0, "print", 0);

        let mut e = TestEngine::over(g);
        let run = e.run_sinks().await;
        assert_eq!(run.logs(), ["35"], "sum = 2 + 5 = 7, mult = 7 * 5 = 35");

        // Moving external state does not recompute: `b` is pure, so its digest
        // is stable and the cached value stands.
        b.store(7, Ordering::Relaxed);
        let run = e.run_sinks().await;
        assert_eq!(run.logs(), ["35"], "a pure node does not re-read the world");

        // Declaring `b` impure re-keys it: now it re-reads on every run.
        e.edit(|g| g.edit_func("b", |func| func.behavior = FuncBehavior::Impure));
        let run = e.run_sinks().await;
        assert_eq!(run.logs(), ["63"], "sum = 2 + 7 = 9, mult = 9 * 7 = 63");
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn required_none_binding_is_stable() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        // sum's first input unbound (required) — sum and downstream can't run.
        e.edit(|g| g.unbind("sum", 0));

        let first = e.run_sinks().await;
        let second = e.run_sinks().await;

        // The schedule is deterministic across runs. What actually *runs* can
        // differ as pure nodes start reusing their cache, but the order cannot
        // flap.
        assert_eq!(first.missing_inputs(), second.missing_inputs());
        assert_eq!(first.missing_inputs(), ["Print", "mult", "sum"]);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn schedule_stable_across_repeated_runs() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());

        // Three runs held at once — an outcome is a snapshot, so they can be
        // compared rather than re-derived.
        let run1 = e.run_sinks().await;
        let run2 = e.run_sinks().await;
        let run3 = e.run_sinks().await;

        // The first run executes everything; once the pure upstream is cached,
        // runs 2 and 3 must schedule identically — this guards the reused
        // per-run buffers being reset cleanly, since a missed reset would drift.
        assert_eq!(run1.ran(), ["get_b", "get_a", "sum", "mult", "Print"]);
        assert_eq!(run2.ran(), ["Print"]);
        assert_eq!(run2.ran(), run3.ran());
        assert_eq!(
            run2.cached(),
            ["get_a", "get_b", "mult", "sum"],
            "everything the second run did not re-run was served from cache"
        );

        // The cached product stays correct every run: sum(1 + 11 = 12) * get_b(11) = 132.
        assert_eq!(e.output_i64("mult", 0), Some(132));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_upstream_output_reused_after_rebinding() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.run_sinks().await;

        // Switch mult to const inputs: its upstream leaves the run.
        e.edit(|g| {
            g.constant("mult", 0, 2i64);
            g.constant("mult", 1, 21i64);
        });
        assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);

        // Switch one back to a bind from the *cached* get_b — mult re-executes,
        // but its producer is served from cache rather than re-run.
        e.edit(|g| g.wire("get_b", 0, "mult", 0));
        assert_eq!(e.run_sinks().await.ran(), ["mult", "Print"]);
        Ok(())
    }

    /// Output buffers are wiped before a re-running node is invoked, so an
    /// unwritten output cannot retain a prior run's value. This sink has no
    /// demanded outputs, so leaving one port `Unbound` is valid.
    #[tokio::test(flavor = "multi_thread")]
    async fn unwritten_output_port_is_cleared_before_reexecution() -> TestResult {
        use crate::async_lambda;

        let invocations = Arc::new(AtomicUsize::new(0));
        let mut g = TestGraph::new();
        g.add("partial_writer", |n| {
            let invocations = invocations.clone();
            n.pure()
                .sink()
                // Const-bound below; changing that const is what re-keys the
                // digest between runs.
                .optional(DataType::Int)
                .output(DataType::Int)
                .output(DataType::Int)
                // Retained so the in-place buffer reuse is observable at all.
                .cache(CacheMode::Ram)
                .lambda(async_lambda!(
                    move |Invocation { outputs, .. }| { invocations = invocations.clone() } => {
                        let run = invocations.fetch_add(1, Ordering::Relaxed);
                        outputs[0] = StaticValue::Int(100 + run as i64).into();
                        if run == 0 {
                            // Only the first run writes the second port.
                            outputs[1] = StaticValue::Int(20).into();
                        }
                        Ok(())
                    }
                ))
        });
        g.constant("partial_writer", 0, 0i64);

        let mut e = TestEngine::over(g);

        let run = e.run_sinks().await;
        assert!(run.errored().is_empty());
        assert_eq!(e.output_i64("partial_writer", 0), Some(100));
        assert_eq!(
            e.output_i64("partial_writer", 1),
            Some(20),
            "run 1 writes both"
        );

        // Invalidate the pure node while keeping its resident buffer available
        // for the next invocation. Run 2 writes only port 0, so port 1 must not
        // retain run 1's value.
        e.edit(|g| g.constant("partial_writer", 0, 1i64));
        let run = e.run_sinks().await;

        assert!(run.errored().is_empty());
        assert_eq!(
            e.output_i64("partial_writer", 0),
            Some(101),
            "port 0 rewritten"
        );
        assert!(
            matches!(e.output("partial_writer", 1), Some(DynamicValue::Unbound)),
            "the unwritten port is cleared before invoke, not left holding 20"
        );
        Ok(())
    }
}

mod node_seeds {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The sample fixture with nothing retained — every node on
    /// `CacheMode::None`, which is what these tests are about.
    fn uncached(hooks: TestFuncHooks) -> TestGraph {
        let mut g = TestGraph::sample_with(hooks);
        g.cache_all(CacheMode::None);
        g
    }

    /// Seeding `sum` runs exactly its cone (`get_a`, `get_b`, `sum`) without
    /// overriding any node's `CacheMode::None` retention policy.
    #[tokio::test]
    async fn seeded_run_executes_only_the_cone_without_retaining_outputs() {
        let mut e = TestEngine::over(uncached(TestFuncHooks {
            get_a: Arc::new(|| Ok(1)),
            get_b: Arc::new(|| 11),
            ..Default::default()
        }));

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
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = |calls: Arc<AtomicUsize>| {
            move || {
                calls.fetch_add(1, Ordering::Relaxed);
            }
        };
        let (a, b) = (calls.clone(), calls.clone());
        let mut e = TestEngine::over(uncached(TestFuncHooks {
            get_a: Arc::new(move || {
                counted(a.clone())();
                Ok(1)
            }),
            get_b: Arc::new(move || {
                counted(b.clone())();
                11
            }),
            ..Default::default()
        }));

        e.run_nodes(["sum"]).await;
        assert_eq!(calls.load(Ordering::Relaxed), 2, "one call to each source");

        let run = e.run_nodes(["sum"]).await;

        assert_eq!(
            calls.load(Ordering::Relaxed),
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
        let mut e = TestEngine::over(uncached(TestFuncHooks {
            get_a: Arc::new(|| Ok(1)),
            get_b: Arc::new(|| 11),
            print: Arc::new(|_| {}),
        }));
        e.edit(|g| g.disable("sum"));

        let run = e
            .run(RunSeeds {
                sinks: true,
                node_ids: vec![e.id("sum")],
                ..Default::default()
            })
            .await
            .expect("the run completes");

        assert_eq!(
            run.ran(),
            ["get_b", "get_a", "sum", "mult", "Print"],
            "the explicit override feeds the ordinary sink during this run"
        );
        assert_eq!(e.output_i64("mult", 0), None, "(1 + 11) * 11, not retained");
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
    /// not something to silently skip. The panicking default hooks prove no
    /// lambda fires.
    #[tokio::test]
    async fn unresolvable_node_seed_fails_the_run() {
        let mut e = TestEngine::over(TestGraph::sample_with(TestFuncHooks::default()));

        let bogus = NodeId::from_u128(0xdead_beef);
        let error = e
            .run(RunSeeds::nodes(vec![bogus]))
            .await
            .expect_err("a stale seed fails the run");

        assert!(matches!(error, Error::NodeSeedNotFound { node_id } if node_id == bogus));
    }
}

mod argument_values {
    use super::*;

    #[test]
    fn nonexistent_node_returns_none() {
        let e = TestEngine::over(TestGraph::sample());

        let nonexistent: NodeId = "00000000-0000-0000-0000-000000000000".into();
        assert!(e.engine.get_argument_values(&nonexistent).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_const_bindings() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample_with(TestFuncHooks {
            get_a: Arc::new(|| unreachable!("const-fed: the sources are never reached")),
            get_b: Arc::new(|| unreachable!("const-fed: the sources are never reached")),
            print: Arc::new(|_| {}),
        }));
        e.edit(|g| {
            g.constant("mult", 0, 3i64);
            g.constant("mult", 1, 5i64);
        });

        e.run_sinks().await;

        assert_eq!(e.input_i64("mult", 0), Some(3));
        assert_eq!(e.input_i64("mult", 1), Some(5));
        assert_eq!(e.outputs("mult").len(), 1);
        assert_eq!(e.output_i64("mult", 0), Some(15), "3 * 5");
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_bound_outputs() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample_with(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 5),
            print: Arc::new(|_| {}),
        }));

        e.run_sinks().await;

        // The two sources emit `Float` (their lambdas cast through `f64`), which
        // the `Int`-declared consumers read through the scalar coercion class —
        // so the variant is worth pinning, not just the number.
        assert!(matches!(
            e.inputs("sum")[0],
            Some(DynamicValue::Static(StaticValue::Float(v))) if v.approximately_eq(2.0)
        ));
        assert!(matches!(
            e.inputs("sum")[1],
            Some(DynamicValue::Static(StaticValue::Float(v))) if v.approximately_eq(5.0)
        ));
        assert_eq!(e.output_i64("sum", 0), Some(7), "2 + 5");

        assert_eq!(e.input_i64("mult", 0), Some(7));
        assert!(matches!(
            e.inputs("mult")[1],
            Some(DynamicValue::Static(StaticValue::Float(v))) if v.approximately_eq(5.0)
        ));
        assert_eq!(e.output_i64("mult", 0), Some(35), "7 * 5");

        assert_eq!(e.input_i64("Print", 0), Some(35));
        assert!(e.outputs("Print").is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_none_binding() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.edit(|g| {
            g.edit_func("mult", |func| func.inputs[1].required = false);
            g.unbind("mult", 1);
        });

        e.run_sinks().await;

        let inputs = e.inputs("mult");
        assert_eq!(inputs.len(), 2);
        assert!(inputs[0].is_some());
        assert!(inputs[1].is_none(), "an unbound port delivers no value");
        Ok(())
    }

    #[test]
    fn before_execution() -> TestResult {
        let e = TestEngine::over(TestGraph::sample());

        // Before execution: all inputs are None (no upstream values yet).
        let inputs = e.inputs("sum");
        assert_eq!(inputs.len(), 2);
        assert!(inputs.iter().all(Option::is_none));
        assert!(e.outputs("sum").is_empty());
        Ok(())
    }
}

mod error_propagation {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn node_error_propagates_to_dependents() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample_with(TestFuncHooks {
            get_a: Arc::new(|| Err(internals::failure("Intentional failure in get_a"))),
            get_b: Arc::new(|| 42),
            print: Arc::new(|_| {}),
        }));

        let run = e.run_sinks().await;

        // The failure and the three consumers that inherit it — errors are
        // reported through the run, not the cross-run cache, which only
        // reflects which outputs survived.
        assert_eq!(run.errored(), ["Print", "get_a", "mult", "sum"]);
        assert!(
            run.error("get_a")
                .expect("the failing node reports its own error")
                .to_string()
                .contains("Intentional failure")
        );
        for name in ["sum", "mult", "Print"] {
            assert!(
                run.error(name)
                    .unwrap_or_else(|| panic!("{name} should carry an upstream error"))
                    .to_string()
                    .contains("upstream"),
                "{name} should report an upstream error",
            );
            assert!(e.outputs(name).is_empty(), "{name} should have no output");
        }

        // The one node off the failing cone keeps its value.
        assert!(e.outputs("get_a").is_empty());
        assert!(run.error("get_b").is_none());
        assert_eq!(e.output_i64("get_b", 0), Some(42));
        Ok(())
    }
}

mod stats {
    use super::*;

    use crate::execution::report::NodeExecutionStatus;

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_inputs_reported() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample());
        e.edit(|g| g.unbind("sum", 0));

        let run = e.run_sinks().await;

        // Port 1 is still bound, so the run names exactly the port that failed
        // rather than flagging the node as a whole.
        assert_eq!(run.missing_ports("sum"), [0]);
        Ok(())
    }

    /// Library drift: wiring that references ports/events the library no
    /// longer declares must still compile — the dangling binding degrades
    /// to unbound (a required input reports missing), a dangling
    /// subscription and pin wire nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn dangling_wiring_compiles_and_reports_missing_input() -> TestResult {
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
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn executed_nodes_reported() -> TestResult {
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
        Ok(())
    }
}

mod events {
    use super::*;
    use crate::async_lambda;
    use crate::graph::func::event::EventLambda;
    use crate::graph::node::special::SpecialNode;

    /// A counter every fixture body below increments, so "how often did this
    /// run" is one shared shape.
    type Calls = Arc<Mutex<i64>>;

    /// An impure source carrying a `tick` event and emitting its own call count.
    fn emitter(calls: Calls) -> impl FnOnce(NodeSpec) -> NodeSpec {
        move |n: NodeSpec| {
            n.output(DataType::Int)
                .event("tick", EventLambda::new(|_state| Box::pin(async move {})))
                .lambda(async_lambda!(
                    move |Invocation { outputs, .. }| { calls = calls.clone() } => {
                        let mut n = calls.lock().await;
                        *n += 1;
                        outputs[0] = StaticValue::Int(*n).into();
                        Ok(())
                    }
                ))
        }
    }

    /// `emit`: impure source with an output and one `tick` event, subscribed to
    /// by `recv`. `recv`: impure consumer bound to emit's output. Neither is a
    /// sink, so only event-driven execution reaches them.
    fn event_pair() -> (TestGraph, Calls) {
        let calls = Arc::new(Mutex::new(0));
        let mut g = TestGraph::new();
        g.add("emit", emitter(calls.clone()));
        g.add("recv", |n| n.records());
        g.subscribe("emit", 0, "recv");
        g.wire("emit", 0, "recv", 0);
        (g, calls)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn execute_events_runs_subscribers() -> TestResult {
        let (g, calls) = event_pair();
        let mut e = TestEngine::over(g);

        let tick = e.event("emit", 0);
        let run = e.run_events([tick]).await;

        // recv subscribes to emit's tick, so recv is the root and emit runs as
        // its dependency.
        assert_eq!(run.ran(), ["emit", "recv"]);
        assert_eq!(*calls.lock().await, 1);
        assert_eq!(run.logs(), ["1"]);
        assert_eq!(
            run.triggered_events,
            [tick],
            "the triggering event is echoed back"
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_sources_collects_nodes_with_subscribers() -> TestResult {
        let (g, calls) = event_pair();
        let mut e = TestEngine::over(g);

        // sinks=false, event_sources=true → emit (which owns a subscribed
        // event) becomes a root; recv is downstream of emit, not a root.
        let run = e.run_event_sources().await;

        assert_eq!(run.ran(), ["emit"]);
        assert_eq!(*calls.lock().await, 1);
        assert!(run.logs().is_empty(), "recv is not reached");
        Ok(())
    }

    /// The bootstrap run re-initializes its event sources every time, bypassing
    /// the cache — the shared state its event lambdas read has to be freshly
    /// built even when the node's digest is unchanged.
    #[tokio::test(flavor = "multi_thread")]
    async fn bootstrap_prepares_events_and_bypasses_source_cache() -> TestResult {
        let (mut g, calls) = event_pair();
        g.edit_func("emit", |func| func.behavior = FuncBehavior::Pure);
        g.cache("emit", CacheMode::Ram);
        let mut e = TestEngine::over(g);

        let tick = e.event("emit", 0);
        for expected in [1, 2] {
            let run = e.run_event_sources().await;
            assert_eq!(
                *calls.lock().await,
                expected,
                "a pure, cached source re-runs"
            );
            assert_eq!(run.armed_events, [tick]);
        }
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bootstrap_prepares_no_events_without_subscribers() -> TestResult {
        let (mut g, _) = event_pair();
        // Drop the subscriber but keep emit reachable by making it a sink.
        g.unsubscribe("emit", 0, "recv");
        g.edit_func("emit", |func| func.sink = true);
        let mut e = TestEngine::over(g);

        let run = e.run_sinks().await;

        assert!(run.ran().contains(&"emit"));
        assert!(
            run.armed_events.is_empty(),
            "emit's event has no subscribers, so nothing is armed"
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_event_source_prepares_no_trigger() -> TestResult {
        let (mut g, _) = event_pair();
        g.edit_func("emit", |func| {
            func.lambda = async_lambda!(|_| {
                Err(InvokeError::external(std::io::Error::other(
                    "bootstrap failed",
                )))
            })
        });
        let mut e = TestEngine::over(g);

        let run = e.run_event_sources().await;

        assert_eq!(run.errored(), ["emit"]);
        assert!(run.armed_events.is_empty());
        Ok(())
    }

    /// A `RunSinks` special node subscribed to an event fires no cone of its own
    /// (it has no ports) — instead firing that event runs *every* sink, exactly
    /// as pressing "Run" would. Here `emit`'s tick reaches only the `RunSinks`
    /// sink, yet the independent `source → sink` cone runs, while `emit`
    /// (neither a sink nor in that cone) does not.
    #[tokio::test(flavor = "multi_thread")]
    async fn run_sinks_node_runs_all_sinks_on_event() -> TestResult {
        let source_calls = Arc::new(Mutex::new(0i64));

        let mut g = TestGraph::new();
        g.add("emit", emitter(Arc::new(Mutex::new(0))));
        g.add("source", emitter(source_calls.clone()));
        g.add("sink", |n| n.records());
        g.add_special("trigger", SpecialNode::RunSinks);
        // The sink's cone (source → sink) is wholly independent of emit.
        g.wire("source", 0, "sink", 0);
        g.subscribe("emit", 0, "trigger");

        let mut e = TestEngine::over(g);
        let run = e.run_events([e.event("emit", 0)]).await;

        // The sink cone ran; emit is neither a sink nor in that cone, so it did
        // not. The `RunSinks` node is itself a sink, so it runs its no-op lambda
        // alongside the promoted sinks rather than seeding a cone of its own.
        // The stack pops the later-declared root first, so the portless
        // trigger settles before the cone it promoted.
        assert_eq!(run.ran(), ["trigger", "source", "sink"]);
        assert_eq!(*source_calls.lock().await, 1);
        assert_eq!(run.logs(), ["1"]);
        assert_eq!(run.triggered_events.len(), 1);
        Ok(())
    }

    /// Without the `RunSinks` sink, firing `emit`'s tick reaches no subscriber,
    /// so the same sink cone is left untouched — isolating the sink as the cause.
    #[tokio::test(flavor = "multi_thread")]
    async fn event_without_run_sinks_sink_runs_nothing() -> TestResult {
        let source_calls = Arc::new(Mutex::new(0i64));

        let mut g = TestGraph::new();
        g.add("emit", emitter(Arc::new(Mutex::new(0))));
        g.add("source", emitter(source_calls.clone()));
        g.add("sink", |n| n.sink().input(DataType::Int));
        g.wire("source", 0, "sink", 0);

        let mut e = TestEngine::over(g);
        let run = e.run_events([e.event("emit", 0)]).await;

        assert!(run.ran().is_empty());
        assert_eq!(*source_calls.lock().await, 0);
        Ok(())
    }
}

mod output_demand {
    use super::*;
    use crate::async_lambda;
    use crate::graph::func::lambda::OutputDemand;

    #[tokio::test(flavor = "multi_thread")]
    async fn unused_output_marked_skip() -> TestResult {
        let seen: Arc<Mutex<Vec<OutputDemand>>> = Arc::new(Mutex::new(Vec::new()));

        let mut g = TestGraph::new();
        g.add("split", |n| {
            let seen = seen.clone();
            n.output(DataType::Int)
                .output(DataType::Int)
                .lambda(async_lambda!(
                    move |Invocation { demand, outputs, .. }| { seen = seen.clone() } => {
                        seen.lock().await.extend_from_slice(demand);
                        outputs[0] = StaticValue::Int(1).into();
                        outputs[1] = StaticValue::Int(2).into();
                        Ok(())
                    }
                ))
        });
        g.add("sink", |n| n.sink().input(DataType::Int));
        // Consume only output 0; output 1 has no consumer.
        g.wire("split", 0, "sink", 0);

        let mut e = TestEngine::over(g);
        e.run_sinks().await;

        assert_eq!(
            e.demand("split"),
            [OutputDemand::Produce, OutputDemand::Skip]
        );
        assert_eq!(e.readers("split"), [1, 0]);
        assert_eq!(
            *seen.lock().await,
            [OutputDemand::Produce, OutputDemand::Skip],
            "the lambda saw the same demand the sweep resolved"
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_node_reruns_when_a_previously_skipped_output_becomes_needed() -> TestResult {
        let calls = Arc::new(Mutex::new(0));
        let received = Arc::new(Mutex::new(Vec::new()));

        let mut g = TestGraph::new();
        g.add("split", |n| {
            let calls = calls.clone();
            n.pure()
                .cache(CacheMode::Ram)
                .output(DataType::Int)
                .output(DataType::Int)
                .lambda(async_lambda!(
                    move |Invocation { demand, outputs, .. }| { calls = calls.clone() } => {
                        *calls.lock().await += 1;
                        if !demand[0].is_skip() {
                            outputs[0] = StaticValue::Int(10).into();
                        }
                        if !demand[1].is_skip() {
                            outputs[1] = StaticValue::Int(20).into();
                        }
                        Ok(())
                    }
                ))
        });
        let sink = |received: Arc<Mutex<Vec<i64>>>| {
            move |n: crate::testing::graph::NodeSpec| {
                n.sink().input(DataType::Int).lambda(async_lambda!(
                    move |Invocation { inputs, .. }| { received = received.clone() } => {
                        received.lock().await.push(inputs[0].as_i64().unwrap());
                        Ok(())
                    }
                ))
            }
        };
        g.add("sink_a", sink(received.clone()));
        g.wire("split", 0, "sink_a", 0);

        let mut e = TestEngine::over(g);
        e.run_sinks().await;

        // A second consumer arrives on the output the first run skipped: the
        // cached value does not cover the new demand, so `split` must re-run.
        e.edit(|g| {
            g.add("sink_b", sink(received.clone()));
            g.wire("split", 1, "sink_b", 0);
        });
        e.run_sinks().await;

        assert_eq!(*calls.lock().await, 2);
        let mut received = received.lock().await.clone();
        received.sort_unstable();
        assert_eq!(received, [10, 10, 20]);
        Ok(())
    }
}

mod topology {
    use super::*;
    use ::common::FloatExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A source emitting `value` and counting how often it was asked.
    fn counted_source(value: i64, calls: Arc<AtomicUsize>) -> impl FnOnce(NodeSpec) -> NodeSpec {
        move |n: NodeSpec| {
            n.pure()
                .cache(CacheMode::Ram)
                .output(DataType::Int)
                .compute(move |_| {
                    calls.fetch_add(1, Ordering::Relaxed);
                    value.into()
                })
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn removing_node_rebuilds_id_keyed_edges() -> TestResult {
        let mut e = TestEngine::over(TestGraph::sample_with(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 5),
            print: Arc::new(|_| {}),
        }));
        assert_eq!(e.engine.compiled().e_nodes.len(), 5);

        // Remove get_b — a middle node feeding sum[1] and mult[1], both optional.
        // The surviving id-keyed bindings must remain valid across the remap.
        e.edit(|g| {
            g.remove("get_b");
        });
        assert_eq!(e.engine.compiled().e_nodes.len(), 4);

        e.run_sinks().await;

        // sum = get_a(2) + none(0) = 2; mult = sum(2) * none(default 1) = 2.
        assert!(matches!(
            e.inputs("sum")[0],
            Some(DynamicValue::Static(StaticValue::Float(v))) if v.approximately_eq(2.0)
        ));
        assert!(e.inputs("sum")[1].is_none());
        assert_eq!(e.output_i64("sum", 0), Some(2));
        assert_eq!(e.output_i64("mult", 0), Some(2));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_graph_executes_cleanly() -> TestResult {
        let mut e = TestEngine::over(TestGraph::new());
        assert!(e.engine.is_empty());

        let run = e.run_sinks().await;

        assert_eq!(run.ran_node_count, 0);
        assert!(run.ran().is_empty());
        assert!(run.errored().is_empty());
        assert!(run.missing_inputs().is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multiple_sinks_all_execute() -> TestResult {
        // Two independent chains: a → print_a, b → print_b.
        let mut g = TestGraph::new();
        g.add("a", |n| n.returns(2i64));
        g.add("b", |n| n.returns(5i64));
        g.add("print_a", |n| n.records());
        g.instance("print_b", "print_a");
        g.wire("a", 0, "print_a", 0);
        g.wire("b", 0, "print_b", 0);

        let mut e = TestEngine::over(g);
        let run = e.run_sinks().await;

        assert_eq!(run.ran_node_count, 4, "both sinks and both sources");
        let mut logged = run.logs();
        logged.sort_unstable();
        assert_eq!(logged, ["2", "5"]);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_output_survives_node_removal() -> TestResult {
        // Both sources are Pure, so their outputs are cached across runs.
        // Removing one chain must preserve the survivor's id-keyed slot.
        let (calls_a, calls_b) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let mut g = TestGraph::new();
        g.add("a", counted_source(2, calls_a.clone()));
        g.add("b", counted_source(5, calls_b.clone()));
        g.add("print_a", |n| n.records());
        g.instance("print_b", "print_a");
        g.wire("a", 0, "print_a", 0);
        g.wire("b", 0, "print_b", 0);

        let mut e = TestEngine::over(g);
        e.run_sinks().await;
        assert_eq!(calls_a.load(Ordering::Relaxed), 1);
        assert_eq!(calls_b.load(Ordering::Relaxed), 1);

        e.edit(|g| {
            g.remove("b");
            g.remove("print_b");
        });
        let run = e.run_sinks().await;

        assert_eq!(
            calls_a.load(Ordering::Relaxed),
            1,
            "the survivor must not recompute after an unrelated node's removal"
        );
        assert!(run.cached().contains(&"a"));
        assert_eq!(e.output_i64("a", 0), Some(2));
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn repeated_structural_churn_stays_correct() -> TestResult {
        // Grow→shrink the graph repeatedly on ONE engine, re-executing each
        // step. Stresses the packed pools and the id-keyed rebuild across many
        // updates (pools grow 2→4 then shrink 4→2 each round).
        let mut g = TestGraph::new();
        g.add("a", |n| n.returns(2i64));
        g.add("print_a", |n| n.records());
        g.wire("a", 0, "print_a", 0);

        let mut e = TestEngine::over(g);
        e.run_sinks().await;

        for round in 0..3 {
            e.edit(|g| {
                g.add("b", |n| n.returns(5i64));
                g.instance("print_b", "print_a");
                g.wire("b", 0, "print_b", 0);
            });
            assert_eq!(e.engine.compiled().e_nodes.len(), 4, "round {round} grow");
            let run = e.run_sinks().await;
            let mut logged = run.logs();
            logged.sort_unstable();
            assert_eq!(logged, ["2", "5"], "round {round} grow values");

            e.edit(|g| {
                g.remove("b");
                g.remove("print_b");
            });
            assert_eq!(e.engine.compiled().e_nodes.len(), 2, "round {round} shrink");
            let run = e.run_sinks().await;
            assert_eq!(run.logs(), ["2"], "round {round} shrink values");
        }
        Ok(())
    }
}

mod graph {
    use super::*;

    /// A func-only graph builds with the node ids unchanged (caches survive).
    #[test]
    fn top_level_func_nodes_keep_identity() {
        let e = TestEngine::over(TestGraph::sample());

        assert_eq!(e.engine.compiled().e_nodes.len(), e.graph.graph.len());
        for node in e.graph.graph.iter() {
            assert!(e.engine.compiled().contains(node.id), "id preserved");
        }
    }
}

/// End-to-end proof that a non-RAM cache mode bounds a run's peak memory: each stage's
/// output is released the instant the next stage consumes it, so only the active frontier is
/// resident at once — the point of the mid-run release.
mod mid_run_release {
    use std::any::Any;
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;

    use super::*;
    use crate::async_lambda;
    use crate::library::TypeEntry;
    use crate::{CustomValue, TypeId};

    const TRACKED_TYPE: &str = "7266406a-8083-4e46-b661-de4308bcec96";

    /// Live/peak count of [`Tracked`] values resident at once during a run.
    #[derive(Debug, Default)]
    struct LiveTracker {
        current: usize,
        peak: usize,
    }

    /// A custom value that registers as live on creation and deregisters on `Drop`, so the
    /// shared [`LiveTracker`] captures the peak number resident simultaneously. Cloning a
    /// `DynamicValue::Custom` clones the `Arc`, not the `Tracked`, so a value stays live until
    /// its last reference (cache slot or invoke buffer) drops — exactly what peak RAM tracks.
    #[derive(Debug)]
    struct Tracked {
        tracker: Arc<StdMutex<LiveTracker>>,
    }

    impl Tracked {
        fn new(tracker: Arc<StdMutex<LiveTracker>>) -> Self {
            {
                let mut t = tracker.lock().unwrap();
                t.current += 1;
                t.peak = t.peak.max(t.current);
            }
            Tracked { tracker }
        }
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.tracker.lock().unwrap().current -= 1;
        }
    }

    impl std::fmt::Display for Tracked {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Tracked")
        }
    }

    impl CustomValue for Tracked {
        fn type_id(&self) -> TypeId {
            TRACKED_TYPE.into()
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
            self
        }
    }

    fn tracked() -> DataType {
        DataType::Custom(TRACKED_TYPE.into())
    }

    /// A pure custom→custom node emitting a fresh [`Tracked`] on every call.
    fn relay(
        tracker: Arc<StdMutex<LiveTracker>>,
        mode: CacheMode,
    ) -> impl FnOnce(NodeSpec) -> NodeSpec {
        move |n: NodeSpec| {
            n.pure()
                .cache(mode)
                .optional(tracked())
                .output(tracked())
                .lambda(async_lambda!(
                    move |Invocation { outputs, .. }| { tracker = tracker.clone() } => {
                        outputs[0] = DynamicValue::Custom(Arc::new(Tracked::new(tracker.clone())));
                        Ok(())
                    }
                ))
        }
    }

    /// A graph with `TRACKED_TYPE` registered, ready for the nodes above.
    fn tracked_graph() -> TestGraph {
        let mut g = TestGraph::new();
        g.library
            .register_type(TRACKED_TYPE, TypeEntry::custom("Tracked"));
        g
    }

    /// Run a 4-stage relay chain into a sink with every relay set to
    /// `relay_mode`, and return the peak number of tracked outputs resident at
    /// once.
    async fn chain_peak(relay_mode: CacheMode) -> usize {
        let tracker = Arc::new(StdMutex::new(LiveTracker::default()));
        let mut g = tracked_graph();
        for stage in 0..4 {
            g.add(&format!("relay{stage}"), relay(tracker.clone(), relay_mode));
        }
        g.add("sink", |n| n.sink().input(tracked()));
        for stage in 1..4 {
            g.wire(
                &format!("relay{}", stage - 1),
                0,
                &format!("relay{stage}"),
                0,
            );
        }
        g.wire("relay3", 0, "sink", 0);

        let mut e = TestEngine::over(g);
        e.run_sinks().await;
        tracker.lock().unwrap().peak
    }

    /// The cache mode drives peak residency. With `None`, each stage's output is freed the
    /// moment the next stage reads it, so only a producer/consumer pair is ever resident →
    /// peak 2, whatever the chain length. With `Ram`, every stage is retained for cross-run
    /// reuse, so all four accumulate → peak 4. That the two differ is the whole feature.
    #[tokio::test]
    async fn none_cache_bounds_peak_residency_but_ram_accumulates() {
        assert_eq!(
            chain_peak(CacheMode::None).await,
            2,
            "None frees each stage the instant it is drained"
        );
        assert_eq!(
            chain_peak(CacheMode::Ram).await,
            4,
            "Ram retains every stage for the whole run"
        );
    }

    /// Each probe's ownership observation, in invocation order, plus what stayed live.
    #[derive(Debug)]
    struct ProbeRun {
        unique_reads: Vec<bool>,
        live_after: usize,
    }

    /// Run `relay → n_probes × probe` with the relay in `relay_mode`.
    ///
    /// A probe takes its input value out of the invoke buffer and records
    /// whether it was uniquely owned (`into_custom` succeeded) — the observable
    /// contract of the executor's move-on-last-use.
    async fn probe_run(relay_mode: CacheMode, probes: usize) -> ProbeRun {
        let tracker = Arc::new(StdMutex::new(LiveTracker::default()));
        let reads = Arc::new(StdMutex::new(Vec::new()));

        let mut g = tracked_graph();
        g.add("relay", relay(tracker.clone(), relay_mode));
        for probe in 0..probes {
            let reads = reads.clone();
            g.add(&format!("probe{probe}"), move |n: NodeSpec| {
                n.sink().input(tracked()).lambda(async_lambda!(
                    move |Invocation { inputs, .. }| { reads = reads.clone() } => {
                        let value = std::mem::take(&mut inputs[0]);
                        reads.lock().unwrap().push(value.into_custom::<Tracked>().is_ok());
                        Ok(())
                    }
                ))
            });
            g.wire("relay", 0, &format!("probe{probe}"), 0);
        }

        // The engine stays bound: dropping it drops its cache, which would
        // release exactly the retained value `live_after` is here to observe.
        let mut e = TestEngine::over(g);
        e.run_sinks().await;
        let unique_reads = reads.lock().unwrap().clone();
        let live_after = tracker.lock().unwrap().current;
        ProbeRun {
            unique_reads,
            live_after,
        }
    }

    /// Move-on-last-use: the last read of a non-RAM output hands the consumer the slot's
    /// own value — uniquely held, so an owning `into_custom` succeeds without a copy — and
    /// nothing stays live after the run. A RAM-cached producer keeps its slot copy, so the
    /// same probe observes a shared value; with fan-out only the final read is the move.
    #[tokio::test]
    async fn last_read_of_non_ram_output_is_uniquely_owned() {
        let run = probe_run(CacheMode::None, 1).await;
        assert_eq!(
            run.unique_reads,
            [true],
            "sole consumer of a None producer owns the value"
        );
        assert_eq!(run.live_after, 0, "moved value dropped with the probe");

        let run = probe_run(CacheMode::Ram, 1).await;
        assert_eq!(
            run.unique_reads,
            [false],
            "the RAM slot keeps a second Arc holder"
        );
        assert_eq!(run.live_after, 1, "the RAM slot retains the value");

        let run = probe_run(CacheMode::None, 2).await;
        assert_eq!(
            run.unique_reads,
            [false, true],
            "with fan-out only the last read is the move"
        );
        assert_eq!(run.live_after, 0, "both probe copies dropped by run end");
    }
}

mod compile_regressions {
    use super::*;
    use crate::graph::output_types::OutputTypes;
    use crate::{FsPathConfig, FsPathMode};

    /// The output pool is range-addressed: when a consumer precedes its producer
    /// in insertion order, lowering claims the producer's *index* early while
    /// output ranges are assigned in emit order — an index-order sequential fill
    /// would hand the two producers each other's types.
    #[test]
    fn output_metadata_follows_ranges_when_consumer_precedes_producer() {
        // Declared consumer-first, then the *other* producer, then the one it
        // binds — so `make_str` claims its index before its range is assigned.
        let mut g = TestGraph::new();
        g.add("sink", |n| n.sink().input(DataType::Any));
        g.add("make_int", |n| n.returns(1i64));
        g.add("make_str", |n| n.returns("s"));
        g.wire("make_str", 0, "sink", 0);

        let e = TestEngine::over(g);
        let program = e.engine.compiled();

        for (name, expected) in [("make_int", DataType::Int), ("make_str", DataType::String)] {
            assert_eq!(
                program.outputs[program.by_id(e.id(name)).outputs][0],
                expected,
                "{name} reads its own type, not its neighbour's"
            );
        }
    }

    /// The authoring-side type at one output port, for the tests that compare
    /// what the editor would paint against what the compiled program carries.
    fn authoring_output_type(g: &TestGraph, name: &str) -> DataType {
        let mut types = OutputTypes::default();
        types.update(&g.graph, &g.library);
        types
            .get(OutputPort::new(g.id(name), 0))
            .cloned()
            .unwrap_or_default()
    }

    #[test]
    fn compiled_output_types_match_authoring_resolution() {
        let path_type = DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFile)));
        let passthrough = |n: NodeSpec| n.input(DataType::Any).wildcard(0);

        let mut g = TestGraph::new();
        g.add("fixed", |n| n.output(DataType::Int));
        // A reroute run long enough that a recursive resolver would blow the
        // stack: the walk must be iterative on both sides.
        let mut previous = "fixed".to_string();
        for hop in 0..70 {
            let name = format!("hop{hop}");
            g.add(&name, passthrough);
            g.wire(&previous, 0, &name, 0);
            previous = name;
        }
        g.add("scalar_const", passthrough);
        g.constant("scalar_const", 0, true);
        g.add("ambiguous_const", passthrough);
        g.constant("ambiguous_const", 0, StaticValue::Enum("A".into()));
        g.add("typed_const", |n| n.input(path_type.clone()).wildcard(0));
        g.constant("typed_const", 0, StaticValue::FsPath("input.fit".into()));
        g.add("unbound", passthrough);

        let cases = [
            ("fixed", DataType::Int),
            ("hop69", DataType::Int),
            ("scalar_const", DataType::Bool),
            ("ambiguous_const", DataType::Any),
            ("typed_const", path_type),
            ("unbound", DataType::Any),
        ];
        let authored: Vec<DataType> = cases
            .iter()
            .map(|(name, _)| authoring_output_type(&g, name))
            .collect();

        let e = TestEngine::over(g);
        let program = e.engine.compiled();
        for ((name, expected), authored) in cases.iter().zip(authored) {
            assert_eq!(&authored, expected, "authoring resolution for {name}");
            assert_eq!(
                &program.outputs[program.by_id(e.id(name)).outputs][0],
                expected,
                "compiled resolution for {name}"
            );
        }
    }

    #[test]
    fn authoring_and_compiled_output_resolution_break_cycles_as_any() {
        let mut g = TestGraph::new();
        g.add("passthrough", |n| n.input(DataType::Any).wildcard(0));
        g.wire("passthrough", 0, "passthrough", 0);
        assert_eq!(authoring_output_type(&g, "passthrough"), DataType::Any);

        // The same wire, compiled: the walk resolves the wildcard through the
        // binding it just interned, and the cycle closes on `Any` there too.
        let e = TestEngine::over(g);
        let program = e.engine.compiled();
        assert_eq!(
            program.outputs[program.by_id(e.id("passthrough")).outputs][0],
            DataType::Any
        );
    }

    /// An install may carry an evolved library: changed inputs and lambdas must
    /// replace their prior compiled forms under the reused lowered node.
    #[tokio::test]
    async fn update_with_evolved_func_recompiles_and_runs_new_lambda() {
        use crate::async_lambda;

        let mut g = TestGraph::new();
        g.add("generate", |n| n.pure().output(DataType::Int).returns(1i64));
        g.add("print", |n| n.records());
        g.wire("generate", 0, "print", 0);

        let mut e = TestEngine::over(g);
        let run = e.run_sinks().await;
        assert_eq!(run.logs(), ["1"], "v1 lambda ran");

        // v2: the same declaration gains an input and a different body.
        e.edit(|g| {
            g.edit_func("generate", |func| {
                func.inputs.push(crate::graph::func::FuncInput::optional(
                    "Extra",
                    DataType::Int,
                ));
                func.lambda = async_lambda!(move |Invocation { outputs, .. }| {
                    outputs[0] = StaticValue::Int(2).into();
                    Ok(())
                });
            })
        });

        assert_eq!(
            e.engine.node_inputs(e.id("generate")).len(),
            1,
            "the reused lowered node picked up the grown input list"
        );
        let run = e.run_sinks().await;
        assert_eq!(
            run.logs(),
            ["2"],
            "the input-shape change re-keyed the digest and the new lambda ran"
        );
    }

    /// A func that grows an **output** must not leave its previous, shorter
    /// snapshot resident.
    ///
    /// The grown-input case above re-keys the digest, which is what retires the
    /// old value. Growing an output need not: the id is unchanged, so `reown`
    /// sees no owner change, and the stale `produced_under` still equals the
    /// stale `current_digest`, so the RAM-retention check keeps a snapshot that
    /// is now one value short of the port list. Debug builds caught it at
    /// install as an `OutputArity` invariant violation; release builds carried
    /// the mismatched snapshot into the run.
    #[tokio::test]
    async fn update_with_a_grown_output_list_retires_the_shorter_snapshot() {
        use crate::async_lambda;

        let body = || {
            async_lambda!(move |Invocation { outputs, .. }| {
                outputs[0] = StaticValue::Int(1).into();
                if outputs.len() > 1 {
                    outputs[1] = StaticValue::Int(2).into();
                }
                Ok(())
            })
        };

        let mut g = TestGraph::new();
        // RAM-cached, so the snapshot is *meant* to survive an install — which
        // is what makes the stale one survive too.
        g.add("generate", |n| {
            n.pure()
                .cache(CacheMode::Ram)
                .output(DataType::Int)
                .lambda(body())
        });
        g.add("print", |n| n.records());
        g.wire("generate", 0, "print", 0);

        let mut e = TestEngine::over(g);
        e.run_sinks().await;

        // The same declaration gains an output. Installing that is where the
        // retained snapshot had to be retired.
        e.edit(|g| {
            g.edit_func("generate", |func| {
                func.outputs
                    .push(crate::graph::func::FuncOutput::new("W", DataType::Int));
            })
        });
        e.run_sinks().await;
    }
}
