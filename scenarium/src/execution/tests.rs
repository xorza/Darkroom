use std::sync::Arc;

use super::*;
use crate::execution::compile::CompileError;
use crate::execution::program::ExecutionBinding;
use crate::graph::{Binding, CacheMode, Graph, InputPort, Node, NodeSearch, OutputPort};
use crate::library::Library;
use crate::node::definition::FuncBehavior;
use crate::node::lambda::OutputDemand;
use crate::testing::{TestFuncHooks, test_func_lib, test_graph};
use crate::{DataType, DynamicValue, StaticValue};
use common::FloatExt;
use tokio::sync::Mutex;

/// Names of the nodes that actually recomputed in the last run, in schedule order.
/// `process_order` now schedules every reachable node (the planner is structural), so
/// this keeps only the *runnable* ones (`wants_execute` — never a `MissingInputs` node)
/// that actually ran (not a reused cache). Before any run `node_ran` is `true` for all,
/// so it reads as "the runnable schedule" for plan-only (`prepare_execution`) tests.
fn execution_node_names_in_order(execution_graph: &ExecutionEngine) -> Vec<String> {
    execution_graph
        .plan
        .process_order
        .iter()
        .filter(|&&node_id| {
            execution_graph.plan.verdicts[&node_id].wants_execute()
                && execution_graph.node_ran(node_id)
        })
        .map(|node_id| {
            execution_graph.compiled.program.e_nodes[node_id]
                .name
                .clone()
        })
        .collect()
}

fn default_hooks() -> TestFuncHooks {
    TestFuncHooks {
        get_a: Arc::new(move || Ok(1)),
        get_b: Arc::new(move || 11),
        print: Arc::new(move |_| {}),
    }
}

/// Instantiate a `Node` for `func_name` with a fixed id; caller wires bindings.
fn node(library: &Library, func_name: &str) -> Node {
    library.by_name(func_name).unwrap().into()
}

/// Set input `idx` of the named node's binding in the source graph.
fn bind(graph: &mut Graph, node_name: &str, idx: usize, binding: Binding) {
    let id = graph
        .find_by_name(node_name, NodeSearch::TopLevel)
        .unwrap()
        .id;
    graph.set_input_binding(InputPort::new(id, idx), binding);
}

mod cache_persistence {
    use super::*;
    use crate::execution::cache::ValueState;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

    /// A unique temp directory removed on drop, so tests don't collide or leak.
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
        use crate::execution::disk_store::DiskStore;
        use crate::library::Library;
        use std::sync::Arc;
        let mut engine = ExecutionEngine::default();
        engine.set_disk_store(DiskStore::new(
            Arc::new(Library::default()),
            Some(dir.0.clone()),
        ));
        engine
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
        let get_a_id = graph
            .find_by_name("get_a", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
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
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            1,
            "the cut prunes the Memory source upstream of a disk-cache hit on reopen"
        );
        assert!(
            !stats.executed_nodes.iter().any(|n| n.node_id == get_a_id),
            "get_a was cut, not executed"
        );
        assert!(
            stats.cached_nodes.contains(&mult_id),
            "mult reused from disk"
        );
        assert!(
            !stats.executed_nodes.iter().any(|n| n.node_id == mult_id),
            "mult did not recompute"
        );

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
            !stats.cached_nodes.contains(&mult_id),
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
        let get_a_id = graph
            .find_by_name("get_a", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
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
            stats.executed_nodes.iter().any(|n| n.node_id == get_a_id),
            "the shared producer runs for its executing consumer"
        );
        assert!(
            stats.cached_nodes.contains(&mult_id),
            "mult still reuses from disk"
        );
    }

    /// Two disk-cached nodes chained (`sum` → `mult`) under an executing sink
    /// (`print`). On reopen only the frontier `mult` — the cached value `print`
    /// actually reads — is deserialized into RAM; the deeper `sum`, whose sole
    /// consumer `mult` is itself reused-from-disk (so never reads it), stays
    /// `ValueState::OnDisk` with no resident bytes. That is the RAM win.
    #[tokio::test]
    async fn chained_disk_cache_loads_only_the_frontier() {
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
        // (RAM + disk) so the frontier the run reads is kept resident, not demoted —
        // that retention is what this test asserts (pure `Disk` would demote it).
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
        let get_a_id = graph
            .find_by_name("get_a", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
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

        // Reopen over the same store with fresh RAM, then run.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        let stats = engine.execute_sinks().await.unwrap();

        // The persist'd sum + mult reuse from disk. `get_a` feeds only the reused `sum` and
        // `mult` (neither reads it), so the pre-run cut prunes it — the `Memory` source is
        // not recomputed on reopen.
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            1,
            "the cut prunes the Memory source feeding only disk-cache hits"
        );
        assert!(
            stats.cached_nodes.contains(&sum_id) && stats.cached_nodes.contains(&mult_id),
            "both disk-cached nodes are reused from disk"
        );

        // The frontier `mult` (read by the executing `print`) is in RAM...
        let mult_resident = engine
            .runtime_slot(engine.by_name("mult").unwrap())
            .output_values()
            .is_some();
        assert!(mult_resident, "frontier cache is loaded into RAM");
        // ...but the deeper `sum` is left on disk: flagged available, never read.
        let sum_resident = engine
            .runtime_slot(engine.by_name("sum").unwrap())
            .output_values()
            .is_some();
        let sum_on_disk = matches!(
            engine.runtime_slot(engine.by_name("sum").unwrap()).value,
            ValueState::OnDisk { .. }
        );
        assert!(
            !sum_resident,
            "a disk cache behind another is not loaded into RAM"
        );
        assert!(
            sum_on_disk,
            "the deeper cache is still flagged available on disk"
        );
    }

    /// Eviction reclaims carried-over RAM: a value a prior run computed and left
    /// resident, that a later run neither executes nor reads as a frontier input, is
    /// dropped from RAM once the disk store can serve it again. In one engine across
    /// two runs, run 1 computes `sum` (disk-cached); run 2 only needs the downstream
    /// `mult` (frontier), so `sum` falls behind the frontier and is reclaimed — while
    /// the still-read `mult` and the non-reloadable `get_a` are kept.
    #[tokio::test]
    async fn prior_run_value_evicted_once_unused_and_reloadable() {
        let dir = TempDir::new("evict");
        let lib = test_func_lib(default_hooks());

        // get_a(1) → sum(Both) = 2 → mult(Both) = 2 → print, one engine. `Both`
        // (RAM + disk) keeps a used value resident and demotes an unused leftover —
        // the retain-vs-evict split this test asserts (pure `Disk` would demote both).
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a"));
        let mut sum = node(&lib, "sum");
        sum.cache = CacheMode::Both;
        graph.add(sum);
        let mut mult = node(&lib, "mult");
        mult.cache = CacheMode::Both;
        graph.add(mult);
        graph.add(node(&lib, "Print"));
        let get_a_id = graph
            .find_by_name("get_a", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
        bind(&mut graph, "sum", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "sum", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "mult", 0, Binding::bind(sum_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &lib).unwrap();

        // Run 1 (cold): everything computes and stays resident; nothing evicted yet
        // (all of it is this run's own output).
        engine.execute_sinks().await.unwrap();
        assert!(
            engine
                .runtime_slot(engine.by_name("sum").unwrap())
                .output_values()
                .is_some(),
            "sum is resident after the run that computed it"
        );

        // Run 2: only `print` runs, reading cached `mult` (frontier). `sum` is now
        // an untouched, reloadable leftover.
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            stats.executed_nodes.len(),
            1,
            "only print runs the second time"
        );

        let sum_resident = engine
            .runtime_slot(engine.by_name("sum").unwrap())
            .output_values()
            .is_some();
        let sum_disk = matches!(
            engine.runtime_slot(engine.by_name("sum").unwrap()).value,
            ValueState::OnDisk { .. }
        );
        let mult_resident = engine
            .runtime_slot(engine.by_name("mult").unwrap())
            .output_values()
            .is_some();
        let get_a_resident = engine
            .runtime_slot(engine.by_name("get_a").unwrap())
            .output_values()
            .is_some();
        assert!(
            !sum_resident,
            "the unused prior-run value is evicted from RAM"
        );
        assert!(
            sum_disk,
            "the evicted value stays available on disk for reload"
        );
        assert!(
            mult_resident,
            "the frontier value the run read is kept resident"
        );
        assert!(
            get_a_resident,
            "a non-reloadable (Memory) value is kept, never force-recomputed"
        );
    }

    /// A top-level node recomputed (rather than reused) in the last run.
    fn ran(stats: &ExecutionStats, id: NodeId) -> bool {
        stats.executed_nodes.iter().any(|e| e.node_id == id)
    }
    /// A top-level node reused a cache (RAM hit, disk hit, or a still-available cut) last run.
    fn cached(stats: &ExecutionStats, id: NodeId) -> bool {
        stats.cached_nodes.contains(&id)
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
        let get_a_id = graph
            .find_by_name("get_a", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
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
        let slot = engine.runtime_slot(engine.by_name("mult").unwrap());
        assert_eq!(
            slot.output_values().is_some(),
            mode.caches_in_ram(),
            "{mode:?}: RAM retention must equal caches_in_ram()"
        );
        match mode {
            CacheMode::None => assert!(
                matches!(slot.value, ValueState::Empty),
                "None drops its value after the run: {:?}",
                slot.value
            ),
            CacheMode::Disk => assert!(
                matches!(slot.value, ValueState::OnDisk { .. }),
                "Disk demotes its RAM copy to disk-only after the run: {:?}",
                slot.value
            ),
            CacheMode::Ram | CacheMode::Both => assert!(
                matches!(
                    slot.output_values().map(|v| &v[0]),
                    Some(DynamicValue::Static(StaticValue::Int(1)))
                ),
                "Ram/Both keep the resident value (1*1=1): {:?}",
                slot.value
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
        let get_a_id = graph
            .find_by_name("get_a", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let a_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        let b_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
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
    /// resident value must not mask the fresh blob. The old three-field slot had
    /// exactly that bug (`hydrate_slot` short-circuited on "values present"); the
    /// `ValueState` enum drops the stale value when `mark_on_disk_if_present` flags
    /// the blob, so the reuse loads the correct bytes.
    ///
    /// The intervening run uses `Ram` mode so it can't overwrite the node's one
    /// disk blob (a `Disk`-mode run would — the blob is keyed by node id).
    #[tokio::test(flavor = "multi_thread")]
    async fn stale_ram_value_does_not_mask_a_valid_disk_blob() -> anyhow::Result<()> {
        let dir = TempDir::new("flip_back");
        let lib = test_func_lib(default_hooks());

        // mult read by print. Const binds detach mult from any upstream, so its
        // digest is a pure function of the two consts. Fixed node ids so the slot
        // (and its resident value) survives each `update`.
        let build = |a: i64, b: i64, mode: CacheMode| {
            let mut graph = Graph::default();
            let mut mult = node(&lib, "mult");
            mult.cache = mode;
            graph.insert(NodeId::from_u128(1), mult);
            graph.insert(NodeId::from_u128(2), node(&lib, "Print"));
            let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
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

        // Flip back to A (Disk): the blob matches the current digest again, but the
        // slot holds 35 in RAM under B's (now superseded) digest. mult is pruned
        // (disk hit) and read as print's frontier — it must serve 6 from disk, not
        // the stale 35.
        engine.update(&build(2, 3, CacheMode::Disk), &lib)?;
        let stats = engine.execute_sinks().await?;

        // mult is served from its disk blob, not recomputed — without this, a recompute
        // would yield 6 regardless and the stale-RAM path would go untested.
        assert!(
            !stats.executed_nodes.iter().any(|n| n.node_id == mult_id),
            "mult is a disk cache hit on flip-back, not recomputed: {:?}",
            stats.executed_nodes
        );

        // `mult` isn't RAM-caching (`Disk` mode) and nothing pins it, so it was
        // reclaimed to its on-disk blob once `Print` finished reading it this
        // run — hydrate it back to inspect the served bytes.
        use crate::execution::query::resolve_node_id;
        let node_id = resolve_node_id(&engine.compiled, &NodeAddress::root(mult_id)).unwrap();
        engine
            .cache
            .hydrate_slot(&engine.compiled.program, node_id)
            .await;
        let vals = engine.get_argument_values(&mult_id).unwrap();
        assert!(
            matches!(vals.outputs[0], DynamicValue::Static(StaticValue::Int(6))),
            "flip-back serves the disk blob (6), not the stale RAM value (35): {:?}",
            vals.outputs
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
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
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

    /// Toggling a node to disk caching persists its *existing* resident value
    /// immediately — via `store_resident_caches`, which the worker runs after every
    /// graph update — without waiting for a re-execution (a cache hit never re-runs).
    #[tokio::test]
    async fn toggling_persist_stores_resident_value_without_a_rerun() {
        let dir = TempDir::new("toggle_persist");
        let lib = test_func_lib(default_hooks());

        // mult(const 2, const 3) = 6 → print, with mult's persistence configurable.
        // Fixed node ids so the slot (and its resident value) survives each update.
        let build = |persist: CacheMode| {
            let mut graph = Graph::default();
            let mut mult = node(&lib, "mult");
            mult.cache = persist;
            graph.insert(NodeId::from_u128(1), mult);
            graph.insert(NodeId::from_u128(2), node(&lib, "Print"));
            let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
            bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(2)));
            bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(3)));
            bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));
            graph
        };

        let mut engine = disk_engine(&dir);

        // Run with Memory caching: mult computes (6) and stays resident, nothing on disk.
        engine.update(&build(CacheMode::Ram), &lib).unwrap();
        engine.execute_sinks().await.unwrap();
        assert!(
            std::fs::read_dir(&dir.0).unwrap().next().is_none(),
            "Memory caching writes nothing to disk"
        );

        // Toggle the node to Disk (a graph edit → update), but do NOT re-run. The
        // resident value must reach disk now, not on some later execution.
        engine.update(&build(CacheMode::Disk), &lib).unwrap();
        engine.store_resident_caches().await;
        assert!(
            std::fs::read_dir(&dir.0).unwrap().next().is_some(),
            "toggling Disk persists the resident value without a re-execution"
        );
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
            let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
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

    /// A corrupt / incompatible cache blob must be *deleted* on a failed load, so the
    /// recompute that follows writes a fresh one. Without the delete, `store_node`'s
    /// skip-if-exists keeps the broken file and the node recomputes on *every* run
    /// (the regression: an old-format blob rejected by `BLOB_FORMAT_VERSION` was never
    /// replaced). Each "session" is a fresh engine, so the disk cache is the only source.
    #[tokio::test]
    async fn corrupt_blob_is_replaced_so_the_next_reopen_is_a_hit() {
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
        let get_a_id = graph
            .find_by_name("get_a", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
        bind(&mut graph, "mult", 0, Binding::bind(get_a_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(get_a_id, 0));
        bind(&mut graph, "Print", 0, Binding::bind(mult_id, 0));

        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
        let ran = |s: &ExecutionStats, id| s.executed_nodes.iter().any(|n| n.node_id == id);

        // Cold run: mult computes and stores its blob.
        {
            let mut engine = disk_engine(&dir);
            engine.update(&graph, &lib).unwrap();
            let stats = engine.execute_sinks().await.unwrap();
            assert!(ran(&stats, mult_id), "cold run computes mult");
        }

        // Corrupt mult's blob *body* (a torn write / an old, version-mismatched format)
        // while keeping the leading 32-byte digest header intact — a garbled header
        // would already fail the presence probe and never reach the on-demand load
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

        // Reopen: the corrupt blob still carries the current digest in its header, so
        // mult is served from disk — but the read fails when `print` pulls it in on
        // demand. `print` is dropped for the run
        // (a node error, not a panic) and the bad blob is deleted, so the next reopen
        // recomputes it fresh.
        {
            let mut engine = disk_engine(&dir);
            engine.update(&graph, &lib).unwrap();
            let stats = engine.execute_sinks().await.unwrap();
            // The consumer reports the real reason — a failed cache load on its input,
            // not "an upstream dependency errored" (no upstream node holds an error).
            let print_id = graph
                .find_by_name("Print", NodeSearch::TopLevel)
                .unwrap()
                .id;
            assert_eq!(stats.node_errors.len(), 1);
            assert_eq!(stats.node_errors[0].node_id, print_id);
            assert!(
                matches!(
                    stats.node_errors[0].error,
                    RunError::InputLoadFailed { input: 0, .. }
                ),
                "the skip carries the input-load reason, got: {:?}",
                stats.node_errors[0].error
            );
        }
        assert!(!blob.exists(), "the corrupt blob was deleted");

        // Reopen: mult's blob is gone → mult recomputes and re-stores; the run succeeds.
        {
            let mut engine = disk_engine(&dir);
            engine.update(&graph, &lib).unwrap();
            let stats = engine.execute_sinks().await.unwrap();
            assert!(
                ran(&stats, mult_id),
                "mult recomputes once its bad blob is gone"
            );
            assert!(stats.node_errors.is_empty(), "the run now succeeds");
        }

        // Reopen: mult's fresh blob is a clean hit → reused, not recomputed.
        {
            let mut engine = disk_engine(&dir);
            engine.update(&graph, &lib).unwrap();
            let stats = engine.execute_sinks().await.unwrap();
            assert!(!ran(&stats, mult_id), "the replaced blob is a clean hit");
        }
    }

    /// A `persist` node whose disk blob is gone by the time the run reaches it must
    /// recompute, not panic. The executor stats for the blob when it processes the node
    /// (`mark_on_disk_if_present`); a missing blob simply misses, so the node runs and
    /// rewrites it — never pruned behind an absent value.
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
        let get_a_id = graph
            .find_by_name("get_a", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
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
        assert!(
            stats.executed_nodes.iter().any(|n| n.node_id == sum_id),
            "sum recomputes when its blob is gone"
        );
        assert!(
            !stats.cached_nodes.contains(&sum_id),
            "a vanished blob is not served as a cache hit"
        );
        assert!(
            recompute.load(Ordering::SeqCst) > after_run1,
            "get_a re-ran to feed sum's recompute"
        );
    }

    /// A redefined output type can't serve a stale blob: `produce`'s func is changed
    /// `Int → Float` with the *same* id+version, but the output signature is folded
    /// into the content digest, so the Float node re-keys away from the Int blob and
    /// recomputes — the consumer sees the correct `Float`, never the stale `Int`.
    #[tokio::test]
    async fn redefined_output_type_rekeys_and_recomputes() {
        use std::sync::Mutex;

        use crate::async_lambda;
        use crate::execution::disk_store::DiskStore;
        use crate::library::Library;
        use crate::node::definition::{Func, FuncInput, FuncOutput};

        const PRODUCE: &str = "63b7a83c-d7fc-46f4-805a-4bf2695e3763";
        const CONSUME: &str = "39bbd6b3-b919-4095-b3d0-79a4515de75e";

        let dir = TempDir::new("wrong-type");
        let produce_runs = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(f64::NAN));

        // `produce` is a pure, Disk-persisted source; its declared output type and
        // value are `Int` when `as_float` is false, `Float` when true — same func id
        // and version, so its digest (which folds neither) is identical either way.
        // `consume` (sink) reads it and records the value as f64.
        let build_lib =
            |as_float: bool| -> Library {
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
                        async_lambda!(move |_, _, _, _, _, outputs| { runs = runs.clone() } => {
                            runs.fetch_add(1, Ordering::SeqCst);
                            outputs[0] = DynamicValue::Static(StaticValue::Float(1.5));
                            Ok(())
                        }),
                    )
                } else {
                    produce.lambda(
                        async_lambda!(move |_, _, _, _, _, outputs| { runs = runs.clone() } => {
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
                    .lambda(async_lambda!(move |_, _, _, inputs, _, _| { recv = recv.clone() } => {
                        *recv.lock().unwrap() = inputs[0].value.as_f64().unwrap_or(f64::NAN);
                        Ok(())
                    })),
            );
                lib
            };

        let engine_with = |lib: Library| {
            let mut eg = ExecutionEngine::default();
            eg.set_disk_store(DiskStore::new(Arc::new(lib), Some(dir.0.clone())));
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
        library.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        // get_b (impure) → mult (persist) → print. mult's cone is impure.
        let mut graph = Graph::default();
        graph.add(node(&library, "get_b"));
        let mut mult = node(&library, "mult");
        mult.cache = CacheMode::Disk;
        graph.add(mult);
        graph.add(node(&library, "Print"));
        let get_b_id = graph
            .find_by_name("get_b", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
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
            !stats.cached_nodes.contains(&mult_id),
            "impure-cone node must not be disk-cached"
        );
        assert!(
            stats.executed_nodes.iter().any(|n| n.node_id == mult_id),
            "mult recomputes on reopen"
        );
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
        let get_a_id = graph
            .find_by_name("get_a", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
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
            !stats.cached_nodes.contains(&mult_id),
            "a Memory-persistence node must not be disk-cached"
        );
        assert!(
            stats.executed_nodes.iter().any(|n| n.node_id == mult_id),
            "mult recomputes on reopen"
        );
    }

    /// The codec guard on `mark_on_disk_if_present`: a `persist` node whose blob is on
    /// disk but whose custom output type has *no registered codec* (a value written by a
    /// build that had the codec, reopened by one that doesn't) is not reused from disk —
    /// it recomputes, rather than being served and then panicking on a failed load. With
    /// the codec it's served from disk instead.
    #[tokio::test]
    async fn missing_codec_skips_disk_cache_instead_of_panicking() {
        use std::any::Any;
        use std::fmt;

        use async_trait::async_trait;

        use crate::CustomValueCodec;
        use crate::async_lambda;
        use crate::library::{Library, TypeEntry};
        use crate::node::definition::{Func, FuncOutput};
        use crate::runtime::context::ContextManager;
        use crate::{CustomValue, TypeId};

        type CodecError = Box<dyn std::error::Error + Send + Sync>;

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
            async fn encode(
                &self,
                value: &dyn CustomValue,
                _ctx: &mut ContextManager,
            ) -> std::result::Result<Vec<u8>, CodecError> {
                Ok(value.as_any().downcast_ref::<Blob>().unwrap().0.clone())
            }
            fn decode(
                &self,
                bytes: Vec<u8>,
            ) -> std::result::Result<Arc<dyn CustomValue>, CodecError> {
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
                        move |_, _, _, _, _, outputs| { counter = recompute.clone() } => {
                            counter.fetch_add(1, Ordering::SeqCst);
                            outputs[0] = DynamicValue::Custom(Arc::new(Blob(vec![9, 9, 9])));
                            Ok(())
                        }
                    )),
            );
            library
        };

        let disk_engine_with_lib = |dir: &TempDir, library: Library| {
            use crate::execution::disk_store::DiskStore;
            let mut engine = ExecutionEngine::default();
            engine.set_disk_store(DiskStore::new(Arc::new(library), Some(dir.0.clone())));
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
        assert!(
            stats.cached_nodes.contains(&blob_id),
            "blob node disk-cached"
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
            !stats.cached_nodes.contains(&blob_id),
            "an undecodable blob is not a cache hit"
        );
        assert!(
            stats.executed_nodes.iter().any(|n| n.node_id == blob_id),
            "the node recomputes instead of tripping a failed frontier load"
        );
    }
}

mod resource_binds {
    use std::any::Any;
    use std::fmt;
    use std::path::PathBuf;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use super::*;
    use crate::async_lambda;
    use crate::library::TypeEntry;
    use crate::node::definition::{Func, FuncInput, FuncOutput};
    use crate::{CustomValue, FsPathConfig, FsPathMode, ResourceStamp, ResourceStamper, TypeId};

    const MAKE_PATH: &str = "be2c3976-3a4f-4ed3-bfe6-8eafb35f084a";
    const LOAD_TEXT: &str = "5abcd2e7-f023-4122-8215-f6305c8b4a7e";
    const ANNOTATE: &str = "b8d6cc90-3c6e-4bdc-aaed-30b6740a9d5d";
    const CAPTURE: &str = "1a9629a9-dfbe-4665-b2b9-6f0d5c21f290";

    /// A temp file path removed on drop.
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
        use crate::execution::disk_store::DiskStore;
        let mut engine = ExecutionEngine::default();
        engine.set_disk_store(DiskStore::new(
            Arc::new(Library::default()),
            Some(dir.0.clone()),
        ));
        engine
    }

    /// The sink both fixtures share: records the received value's text.
    fn capture_func(captured: Arc<StdMutex<String>>) -> Func {
        Func::new(CAPTURE, "capture")
            .category("Test")
            .sink()
            .input(FuncInput::required("Value", DataType::Any))
            .lambda(async_lambda!(
                move |_, _, _, inputs, _, _| { captured = captured.clone() } => {
                    *captured.lock().unwrap() =
                        inputs[0].value.as_string().unwrap_or_default().to_string();
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
                .lambda(async_lambda!(move |_, _, _, inputs, _, outputs| {
                    let path = inputs[0].value.as_string().unwrap().to_string();
                    outputs[0] = StaticValue::FsPath(path).into();
                    Ok(())
                })),
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
                    move |_, _, _, inputs, _, outputs| { loads = loads.clone() } => {
                        loads.fetch_add(1, Ordering::SeqCst);
                        let path = inputs[0].value.as_fs_path().unwrap().to_string();
                        let text = std::fs::read_to_string(&path).map_err(anyhow::Error::from)?;
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
                    move |_, _, _, inputs, _, outputs| { annotates = annotates.clone() } => {
                        annotates.fetch_add(1, Ordering::SeqCst);
                        let text = inputs[0].value.as_string().unwrap();
                        outputs[0] = StaticValue::String(format!("[{text}]")).into();
                        Ok(())
                    }
                )),
        );
        lib.add(capture_func(captured));
        lib
    }

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
        let make_id = graph
            .find_by_name("make_path", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let load_id = graph
            .find_by_name("load_text", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let annotate_id = graph
            .find_by_name("annotate", NodeSearch::TopLevel)
            .unwrap()
            .id;
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

    fn ran(stats: &ExecutionStats, id: NodeId) -> bool {
        stats.executed_nodes.iter().any(|n| n.node_id == id)
    }

    /// The core regression: a path arriving over a **Bind** edge keys the loader on the
    /// file behind the *delivered value*. Editing the file re-keys and recomputes the
    /// loader (pre-fix the chain's digests never changed, so the stale decode was served
    /// forever), while an unchanged file still reuses the cache — the reach-time re-stamp
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
        assert!(stats.cached_nodes.contains(&fx.load_id));
        assert_eq!(
            annotates.load(Ordering::SeqCst),
            1,
            "downstream of the late-stamped loader skips compute on its hit"
        );
        assert!(stats.cached_nodes.contains(&fx.annotate_id));

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
        assert!(stats.cached_nodes.contains(&fx.load_id));
        assert!(!ran(&stats, fx.load_id));
        assert_eq!(
            annotates.load(Ordering::SeqCst),
            1,
            "downstream of the late-stamped loader is a disk hit too"
        );
        assert!(stats.cached_nodes.contains(&fx.annotate_id));
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

    const STORE_TYPE: &str = "cbedef18-3a26-4a61-bdf4-5ec651e304d9";
    const MAKE_HANDLE: &str = "94dcaefc-c7aa-40ea-a297-33bfbfe68f72";
    const READ_STORE: &str = "bf9e0a40-a2ad-411c-8144-a8ae8f5ab491";

    /// The external state a [`StoreHandle`] names: versioned mutable content — the
    /// referent the stamper folds and the reader dereferences.
    #[derive(Debug, Default)]
    struct Store {
        version: AtomicU64,
        content: StdMutex<String>,
    }

    /// A custom resource-reference value: names the store, contains nothing.
    #[derive(Debug)]
    struct StoreHandle;
    impl fmt::Display for StoreHandle {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "StoreHandle")
        }
    }
    impl CustomValue for StoreHandle {
        fn type_id(&self) -> TypeId {
            STORE_TYPE.into()
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
            self
        }
    }

    #[derive(Debug)]
    struct StoreStamper(Arc<Store>);
    impl ResourceStamper for StoreStamper {
        fn stamp(&self, _value: &DynamicValue) -> ResourceStamp {
            ResourceStamp::from_bytes(&self.0.version.load(Ordering::SeqCst).to_le_bytes())
        }
    }

    /// End-to-end wiring of a **registered** resource type: `TypeEntry::with_stamper` →
    /// flatten resolves the input's stamper → digest folds the referent version →
    /// reach-time re-stamp keeps the reader cacheable. Bumping the store's version is the
    /// only change between runs — no value, const, or structural digest moves — and the
    /// reader must recompute exactly then.
    #[tokio::test]
    async fn registered_resource_type_rekeys_reader_on_referent_change() {
        let store = Arc::new(Store {
            version: AtomicU64::new(1),
            content: StdMutex::new("v1".into()),
        });
        let reads = Arc::new(AtomicUsize::new(0));
        let captured = Arc::new(StdMutex::new(String::new()));

        let mut lib = Library::default();
        lib.register_type(
            STORE_TYPE,
            TypeEntry::custom("StoreHandle").with_stamper(Arc::new(StoreStamper(store.clone()))),
        );
        lib.add(
            Func::new(MAKE_HANDLE, "make_handle")
                .category("Test")
                .pure()
                .output(FuncOutput::new(
                    "Handle",
                    DataType::Custom(STORE_TYPE.into()),
                ))
                .lambda(async_lambda!(move |_, _, _, _, _, outputs| {
                    outputs[0] = DynamicValue::from_custom(StoreHandle);
                    Ok(())
                })),
        );
        let (lambda_store, lambda_reads) = (store.clone(), reads.clone());
        lib.add(
            Func::new(READ_STORE, "read_store")
                .category("Test")
                .pure()
                .input(FuncInput::required(
                    "Handle",
                    DataType::Custom(STORE_TYPE.into()),
                ))
                .output(FuncOutput::new("Text", DataType::String))
                .lambda(async_lambda!(
                    move |_, _, _, _inputs, _, outputs| {
                        store = lambda_store.clone(),
                        reads = lambda_reads.clone()
                    } => {
                        reads.fetch_add(1, Ordering::SeqCst);
                        outputs[0] =
                            StaticValue::String(store.content.lock().unwrap().clone()).into();
                        Ok(())
                    }
                )),
        );
        lib.add(capture_func(captured.clone()));

        // make_handle(Ram) → read_store(Ram) → capture.
        let mut graph = Graph::default();
        let mut make = node(&lib, "make_handle");
        make.cache = CacheMode::Ram;
        graph.add(make);
        let mut read = node(&lib, "read_store");
        read.cache = CacheMode::Ram;
        graph.add(read);
        graph.add(node(&lib, "capture"));
        let make_id = graph
            .find_by_name("make_handle", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let read_id = graph
            .find_by_name("read_store", NodeSearch::TopLevel)
            .unwrap()
            .id;
        bind(&mut graph, "read_store", 0, Binding::bind(make_id, 0));
        bind(&mut graph, "capture", 0, Binding::bind(read_id, 0));

        let mut engine = ExecutionEngine::default();
        engine.update(&graph, &lib).unwrap();

        // Cold run computes; a second run with the referent unchanged is a cache hit.
        engine.execute_sinks().await.unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(*captured.lock().unwrap(), "v1");
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "unchanged referent ⇒ the reader stays cached"
        );
        assert!(stats.cached_nodes.contains(&read_id));

        // Mutate the referent: version bump + new content. Nothing structural changed —
        // only the stamper sees it — and the reader recomputes while the handle producer
        // stays a RAM hit.
        store.version.store(2, Ordering::SeqCst);
        *store.content.lock().unwrap() = "v2".into();
        let stats = engine.execute_sinks().await.unwrap();
        assert_eq!(
            reads.load(Ordering::SeqCst),
            2,
            "a referent version bump re-keys the reader"
        );
        assert_eq!(*captured.lock().unwrap(), "v2");
        assert!(!ran(&stats, make_id), "the handle producer stays cached");
        assert!(ran(&stats, read_id));
    }
}

mod graph_structure {
    use super::*;

    #[test]
    fn basic_run() -> anyhow::Result<()> {
        let graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "Print"]
        );

        assert_eq!(execution_graph.compiled.program.e_nodes.len(), 5);
        assert_eq!(execution_graph.plan.process_order.len(), 5);
        assert!(
            execution_graph
                .compiled
                .program
                .node_ids()
                .all(|node_id| !execution_graph.plan.verdicts[&node_id].missing_required_inputs())
        );
        assert!(
            execution_graph
                .compiled
                .program
                .node_ids()
                .all(|node_id| execution_graph.plan.verdicts[&node_id].wants_execute())
        );

        let get_a = execution_graph.by_name("get_a").unwrap();
        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("Print").unwrap();

        // get_a→sum[0], get_b→sum[1]+mult[1], sum→mult[0], mult→print[0]
        assert_eq!(
            execution_graph.node_output_demand(get_a)[0],
            OutputDemand::Produce
        );
        assert_eq!(
            execution_graph.node_output_demand(get_b)[0],
            OutputDemand::Produce
        );
        assert_eq!(
            execution_graph.node_output_demand(sum)[0],
            OutputDemand::Produce
        );
        assert_eq!(
            execution_graph.node_output_demand(mult)[0],
            OutputDemand::Produce
        );
        assert_eq!(execution_graph.node_output_readers(get_a), &[1]);
        assert_eq!(execution_graph.node_output_readers(get_b), &[2]);
        assert_eq!(execution_graph.node_output_readers(sum), &[1]);
        assert_eq!(execution_graph.node_output_readers(mult), &[1]);

        assert!(print.sink);

        Ok(())
    }

    #[test]
    fn updates_after_graph_change() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &library).unwrap();

        // Rewire mult to get_a and get_b directly (bypassing sum)
        let binding1 = Binding::bind(
            graph
                .find_by_name("get_a", NodeSearch::TopLevel)
                .unwrap()
                .id,
            0,
        );
        let binding2 = Binding::bind(
            graph
                .find_by_name("get_b", NodeSearch::TopLevel)
                .unwrap()
                .id,
            0,
        );
        bind(&mut graph, "mult", 0, binding1);
        bind(&mut graph, "mult", 1, binding2);

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        let get_a = execution_graph.by_name("get_a").unwrap();
        let get_b = execution_graph.by_name("get_b").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("Print").unwrap();

        assert_eq!(execution_graph.node_output_demand(get_a).len(), 1);
        assert_eq!(execution_graph.node_output_demand(get_b).len(), 1);
        assert_eq!(execution_graph.node_output_demand(mult).len(), 1);
        assert!(execution_graph.node_output_demand(print).is_empty());
        // Now each source has exactly 1 consumer (sum is no longer in the path)
        assert_eq!(
            execution_graph.node_output_demand(get_a)[0],
            OutputDemand::Produce
        );
        assert_eq!(
            execution_graph.node_output_demand(get_b)[0],
            OutputDemand::Produce
        );
        assert_eq!(
            execution_graph.node_output_demand(mult)[0],
            OutputDemand::Produce
        );
        assert_eq!(execution_graph.node_output_readers(get_a), &[1]);
        assert_eq!(execution_graph.node_output_readers(get_b), &[1]);
        assert_eq!(execution_graph.node_output_readers(mult), &[1]);

        Ok(())
    }

    #[test]
    fn update_rejects_func_missing_from_lib_and_keeps_prior_program() {
        let graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        // A good compile establishes a program.
        execution_graph.update(&graph, &library).unwrap();
        assert_eq!(execution_graph.compiled.program.e_nodes.len(), 5);

        // Re-compiling the same graph against a library that defines none of
        // its funcs is rejected with a message naming a missing func.
        let CompileError { message } = execution_graph
            .update(&graph, &Library::default())
            .unwrap_err();
        assert!(
            message.contains("absent from the library"),
            "message should explain the missing func, got: {message}"
        );

        // The rejection happens before any mutation, so the prior program is
        // left intact rather than torn down.
        assert_eq!(execution_graph.compiled.program.e_nodes.len(), 5);
    }
}

mod missing_inputs {
    use super::*;

    #[test]
    fn required_missing_propagates_downstream() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());

        // Remove sum's first input binding (required by default)
        bind(&mut graph, "sum", 0, Binding::None);

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("Print").unwrap();

        // get_b has no missing inputs (no inputs at all)
        assert!(
            !execution_graph
                .node_verdict(get_b)
                .missing_required_inputs()
        );
        // sum is missing input[0], propagates to downstream mult and print — so none of
        // them is runnable (get_b, a source with satisfied inputs, still is).
        for gated in [sum, mult, print] {
            assert!(
                execution_graph
                    .node_verdict(gated)
                    .missing_required_inputs()
            );
            assert!(!execution_graph.node_verdict(gated).wants_execute());
        }

        Ok(())
    }

    /// A *binding* to a missing-required producer propagates even through an
    /// **optional** input: the wired value can't be delivered, so the consumer
    /// (and its consumers) are missing too. Optionality only excuses an
    /// *unbound* input (see `optional_unbound_does_not_propagate`), not a
    /// binding to a broken upstream.
    #[test]
    fn optional_bind_to_missing_propagates() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut library = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        // sum missing-required; mult[0] stays bound to sum but is made optional.
        bind(&mut graph, "sum", 0, Binding::None);
        library.by_name_mut("mult").unwrap().inputs[0].required = false;

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("Print").unwrap();

        // The missing flag flows through the optional bind to mult and on to print, so
        // the gated chain isn't runnable (its sources still are).
        for gated in [sum, mult, print] {
            assert!(
                execution_graph
                    .node_verdict(gated)
                    .missing_required_inputs()
            );
            assert!(!execution_graph.node_verdict(gated).wants_execute());
        }

        Ok(())
    }

    /// The contrast to `optional_bind_to_missing_propagates`: an optional input
    /// left **unbound** is a deliberate no-value, so it does not flag the node
    /// missing — it runs with its default.
    #[test]
    fn optional_unbound_does_not_propagate() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut library = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        // mult[0] unbound + optional (not wired to anything).
        bind(&mut graph, "mult", 0, Binding::None);
        library.by_name_mut("mult").unwrap().inputs[0].required = false;

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("Print").unwrap();

        assert!(!execution_graph.node_verdict(mult).missing_required_inputs());
        assert!(
            !execution_graph
                .node_verdict(print)
                .missing_required_inputs()
        );
        assert!(execution_node_names_in_order(&execution_graph).contains(&"mult".to_string()));

        Ok(())
    }

    /// Executing counterpart: an optional bind to a gated upstream gates the
    /// consumer chain, so the executor never reads the absent output. Regression
    /// for the worker panicking in `collect_inputs` ("missing output values") —
    /// the planned-only siblings above can't catch it since they never execute.
    #[tokio::test(flavor = "multi_thread")]
    async fn optional_bind_to_gated_upstream_is_gated() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut library = test_func_lib(default_hooks());

        let get_b_id = graph
            .find_by_name("get_b", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;

        // sum's required input[0] unbound → sum missing-required → gated.
        bind(&mut graph, "sum", 0, Binding::None);
        // mult[0] (required) gets a real value; mult[1] is the only bind to the
        // gated sum and is *optional* — so this exercises optional-bind
        // propagation specifically. mult and print end up gated.
        bind(&mut graph, "mult", 0, Binding::bind(get_b_id, 0));
        bind(&mut graph, "mult", 1, Binding::bind(sum_id, 0));
        library.by_name_mut("mult").unwrap().inputs[1].required = false;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        // Pre-fix, this panicked the worker; now the chain is gated and nothing runs.
        execution_graph.execute_sinks().await?;

        // The run completes (no panic reading sum's absent output); the gated `mult`
        // never runs, so it never reads that value.
        let mult = execution_graph.by_name("mult").unwrap();
        assert!(execution_graph.node_verdict(mult).missing_required_inputs());
        assert!(!execution_node_names_in_order(&execution_graph).contains(&"mult".to_string()));

        Ok(())
    }
}

mod disabled_nodes {
    use super::*;

    /// Disabling `sum` drops it from the program entirely, and its
    /// consumer `mult` (whose input[0] was bound to sum) sees that wire as
    /// unbound — so the missing-required-input flag propagates downstream
    /// exactly as if the binding had been cleared.
    #[test]
    fn disabled_node_skipped_and_breaks_downstream() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());

        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        graph
            .find_mut(&sum_id, NodeSearch::TopLevel)
            .unwrap()
            .disabled = true;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        // The disabled node emits no execution node at all.
        assert!(
            execution_graph.by_name("sum").is_none(),
            "disabled node must be absent from the program"
        );

        // get_b has no inputs, so it's unaffected; mult/print lost their
        // (transitive) producer and are flagged missing-required-input.
        let get_b = execution_graph.by_name("get_b").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("Print").unwrap();
        assert!(
            !execution_graph
                .node_verdict(get_b)
                .missing_required_inputs()
        );
        assert!(execution_graph.node_verdict(mult).missing_required_inputs());
        assert!(
            execution_graph
                .node_verdict(print)
                .missing_required_inputs()
        );

        Ok(())
    }

    /// With `mult`'s sum-fed input made optional, disabling `sum` no longer
    /// breaks the chain: `sum` is skipped but `get_b → mult → print` still
    /// runs (mirrors `non_required_missing_does_not_propagate`, but via the
    /// disable flag rather than a cleared binding).
    #[test]
    fn disabled_upstream_with_optional_consumer_still_runs() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut library = test_func_lib(TestFuncHooks::default());

        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        graph
            .find_mut(&sum_id, NodeSearch::TopLevel)
            .unwrap()
            .disabled = true;
        library.by_name_mut("mult").unwrap().inputs[0].required = false;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        assert!(execution_graph.by_name("sum").is_none());
        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "mult", "Print"]
        );

        Ok(())
    }
}

mod const_bindings {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn const_binding_tracks_changes() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let library = test_func_lib(default_hooks());
        let mut execution_graph = ExecutionEngine::default();

        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(3)));
        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(5)));

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        // Only mult and print execute — the const binds detach mult from its
        // upstream, so get_a/get_b/sum are pruned.
        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "Print"]
        );

        let mult_id = execution_graph.by_name("mult").unwrap().id;
        let print_id = execution_graph.by_name("Print").unwrap().id;
        let ran = |stats: &ExecutionStats, id| stats.executed_nodes.iter().any(|n| n.node_id == id);

        // Re-run with the same bindings: mult's digest is unchanged, so it's reused
        // (cache hit); only print (impure sink) actually recomputes.
        execution_graph.update(&graph, &library).unwrap();
        let stats = execution_graph.execute_sinks().await?;
        assert!(stats.cached_nodes.contains(&mult_id), "mult reused");
        assert!(!ran(&stats, mult_id), "mult did not recompute");
        assert!(ran(&stats, print_id), "print recomputes");

        // Change one const: mult's digest changes ⇒ cache miss ⇒ it re-executes.
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(4)));
        execution_graph.update(&graph, &library).unwrap();
        let stats = execution_graph.execute_sinks().await?;
        assert!(ran(&stats, mult_id), "a const change recomputes mult");
        assert!(ran(&stats, print_id));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn const_binding_invokes_only_once() -> anyhow::Result<()> {
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || unreachable!()),
            get_b: Arc::new(move || unreachable!()),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(3)));
        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(5)));

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "Print"]
        );

        // Same const value: no re-execution of mult
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(3)));
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["Print"]);

        // Different const value: mult re-executes
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(4)));
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "Print"]
        );

        // Stable again
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["Print"]);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn const_excludes_upstream_node() -> anyhow::Result<()> {
        let library = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        // Replace sum[0] (get_a) with a const — get_a is no longer needed
        bind(&mut graph, "sum", 0, Binding::Const(33.into()));

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "sum", "mult", "Print"]
        );

        // Also unbind sum[1] — now sum has all const/none inputs, no upstream needed
        bind(&mut graph, "sum", 1, Binding::None);

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["sum", "mult", "Print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn change_from_const_to_bind_recomputes() -> anyhow::Result<()> {
        let library = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        let get_b_id = graph
            .find_by_name("get_b", NodeSearch::TopLevel)
            .unwrap()
            .id;
        bind(&mut graph, "sum", 0, Binding::Const(33.into()));

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "sum", "mult", "Print"]
        );

        // Switch from const back to bind — sum must re-execute
        bind(&mut graph, "sum", 0, Binding::bind(get_b_id, 0));

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["sum", "mult", "Print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn optional_input_binding_change_recomputes() -> anyhow::Result<()> {
        let library = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        // Switch mult inputs to const/none
        bind(&mut graph, "mult", 0, Binding::Const(2.into()));
        bind(&mut graph, "mult", 1, Binding::None);

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "Print"]
        );

        // Stable on rerun
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["Print"]);

        Ok(())
    }
}

mod behavior {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn pure_node_skips_on_rerun() -> anyhow::Result<()> {
        // `get_b` is a pure source in the fixture, so once its output is cached its
        // digest is unchanged on a re-run and it reuses that value rather than running.
        let graph = test_graph();
        let library = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();

        // First run: get_b (pure source) executes.
        execution_graph.execute_sinks().await?;
        assert!(execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

        // Re-run: get_b's digest is unchanged, so it reuses its RAM output — skipped.
        execution_graph.execute_sinks().await?;
        assert!(!execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn default_node_skips_on_rerun() -> anyhow::Result<()> {
        let graph = test_graph();
        let library = test_func_lib(default_hooks());
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "Print"]
        );

        // Second run: only print (impure sink) re-executes, others cached
        let exe_stats = execution_graph.execute_sinks().await?;
        assert_eq!(execution_node_names_in_order(&execution_graph), ["Print"]);
        assert_eq!(exe_stats.cached_nodes.len(), 4);

        // Cached mult must still hold the correct product, not a stale value:
        // sum = get_a(1) + get_b(11) = 12; mult = 12 * get_b(11) = 132
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
        let vals = execution_graph.get_argument_values(&mult_id).unwrap();
        assert!(matches!(
            vals.outputs[0],
            DynamicValue::Static(StaticValue::Int(132))
        ));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn execute_emits_started_then_finished_progress_per_node() -> anyhow::Result<()> {
        use crate::execution::report::{RunEvent, RunPhase};
        use tokio::sync::mpsc::unbounded_channel;

        let graph = test_graph();
        let library = test_func_lib(default_hooks());
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        let (tx, mut rx) = unbounded_channel::<RunEvent>();
        let stats = eg
            .execute(
                RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                Some(&tx),
                CancelToken::never(),
            )
            .await?;
        drop(tx);

        let mut events: Vec<(NodeId, RunPhase)> = Vec::new();
        while let Ok(e) = rx.try_recv() {
            let RunEvent::Progress(p) = e else {
                continue;
            };
            events.push((p.node_id, p.phase));
        }

        let name_of: std::collections::HashMap<NodeId, String> =
            ["get_a", "get_b", "sum", "mult", "Print"]
                .iter()
                .map(|n| {
                    (
                        graph.find_by_name(n, NodeSearch::TopLevel).unwrap().id,
                        n.to_string(),
                    )
                })
                .collect();

        // Events come in Started→Finished pairs for the *same* node (the
        // executor is sequential, so each node brackets before the next starts).
        assert_eq!(events.len() % 2, 0, "paired events");
        let mut started_order: Vec<String> = Vec::new();
        for pair in events.chunks_exact(2) {
            let (sid, sphase) = pair[0];
            let (fid, fphase) = pair[1];
            assert!(
                matches!(sphase, RunPhase::Started { .. }),
                "first of pair is Started",
            );
            assert_eq!(sid, fid, "Started/Finished are the same node");
            assert!(
                matches!(fphase, RunPhase::Finished { elapsed_secs } if elapsed_secs >= 0.0),
                "second of pair is Finished with non-negative elapsed",
            );
            started_order.push(name_of[&sid].clone());
        }

        // The progressed order equals the executor's recorded run order, and
        // covers exactly the finally-executed nodes.
        assert_eq!(started_order, execution_node_names_in_order(&eg));
        assert_eq!(started_order.len(), stats.executed_nodes.len());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn execute_honors_cancel_flag_and_marks_cancelled() -> anyhow::Result<()> {
        use common::CancelToken;

        let graph = test_graph();
        let library = test_func_lib(default_hooks());
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // Pre-tripped: the executor breaks at the first loop-top check, so no
        // node runs and the run is flagged cancelled.
        let tripped = CancelToken::new();
        tripped.cancel();
        let stats = eg
            .execute(
                RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                None,
                tripped,
            )
            .await?;
        assert!(stats.cancelled, "pre-tripped run is cancelled");
        assert!(
            stats.executed_nodes.is_empty(),
            "no node runs when cancel is already set"
        );

        // A fresh, un-cancelled token runs the whole graph (nothing cached from
        // the aborted run above).
        let stats = eg
            .execute(
                RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                None,
                CancelToken::new(),
            )
            .await?;
        assert!(!stats.cancelled);
        assert_eq!(
            stats.executed_nodes.len(),
            5,
            "all nodes run when not cancelled"
        );

        Ok(())
    }

    /// A node cancelled *mid-invoke* (the run is cancelled while its lambda
    /// runs) must not be reported executed and must not cache its partial
    /// output — otherwise the next run treats it as already computed. Models
    /// "start a run, immediately cancel it": the in-flight node bails with `Ok`
    /// but its result is bogus.
    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_mid_invoke_drops_in_flight_node_and_reruns() -> anyhow::Result<()> {
        use std::sync::atomic::{AtomicBool, Ordering};

        use common::CancelToken;

        use crate::async_lambda;
        use crate::execution::stats::NodeError;
        use crate::graph::{Graph, NodeId};
        use crate::library::Library;
        use crate::node::definition::{Func, FuncOutput};

        // Trips the cancel on its first invoke only, so the re-run completes.
        let cancel_first = Arc::new(AtomicBool::new(true));
        let library: Library = [Func::new("8400cb3a-a5d2-4fcd-a9d8-0ab4880c710f", "self_cancel")
            .category("Debug")
            .pure()
            .sink()
            .output(FuncOutput::new("out", DataType::Int))
            .lambda(async_lambda!(
                move |ctx, _, _, _, _, outputs| { cancel_first = Arc::clone(&cancel_first) } => {
                    if cancel_first.swap(false, Ordering::Relaxed) {
                        // Stand in for the user hitting Cancel while this node runs.
                        ctx.cancel_flag().cancel();
                    }
                    outputs[0] = DynamicValue::Static(StaticValue::Int(7));
                    Ok(())
                }
            ))]
        .into();

        let mut graph = Graph::default();
        let node_id: NodeId = "acb11422-9951-4fc6-9696-53b1a6699120".into();
        let node: Node = library.by_name("self_cancel").unwrap().into();
        graph.insert(node_id, node);
        graph.debug_check();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // Run 1: the node trips the cancel mid-invoke — it must not appear as
        // executed (it didn't complete), and the run is flagged cancelled.
        let stats = eg
            .execute(
                RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                None,
                CancelToken::new(),
            )
            .await?;
        assert!(stats.cancelled, "the node cancelled the run mid-invoke");
        assert!(
            stats.executed_nodes.is_empty(),
            "an in-flight cancelled node is not reported executed (no green glow)"
        );
        assert!(
            matches!(
                stats.node_errors.as_slice(),
                [NodeError { node_id: n, error: RunError::Cancelled { .. } }] if *n == node_id
            ),
            "the node is reported truthfully as Cancelled, not a fake success: {:?}",
            stats.node_errors
        );

        // Run 2: a fresh token. The node's partial output was dropped, so it
        // re-executes rather than being served from a bogus cache.
        let stats = eg
            .execute(
                RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                None,
                CancelToken::new(),
            )
            .await?;
        assert!(!stats.cancelled);
        assert_eq!(
            stats.executed_nodes.len(),
            1,
            "the cancelled node re-runs next time (its output was not cached)"
        );
        assert!(
            stats.cached_nodes.is_empty(),
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
    async fn lambda_cancelled_error_maps_to_error_cancelled() -> anyhow::Result<()> {
        use crate::async_lambda;
        use crate::execution::stats::NodeError;
        use crate::graph::{Graph, NodeId};
        use crate::library::Library;
        use crate::node::definition::{Func, FuncOutput};
        use crate::node::lambda::InvokeError;

        let library: Library = [
            Func::new("8003e30b-0417-474d-a77f-1d3ea71ac6b3", "always_cancel")
                .category("Debug")
                .pure()
                .sink()
                .output(FuncOutput::new("out", DataType::Int))
                .lambda(async_lambda!(move |_, _, _, _, _, _| {
                    Err(InvokeError::Cancelled)
                })),
        ]
        .into();

        let mut graph = Graph::default();
        let node_id: NodeId = "c791f8aa-3bf9-435d-8530-f3904b4b6a28".into();
        let node: Node = library.by_name("always_cancel").unwrap().into();
        graph.insert(node_id, node);
        graph.debug_check();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        let stats = eg
            .execute(
                RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                None,
                common::CancelToken::new(),
            )
            .await?;

        assert!(
            stats.executed_nodes.is_empty(),
            "a cancelled lambda is not reported executed"
        );
        assert!(
            matches!(
                stats.node_errors.as_slice(),
                [NodeError { node_id: n, error: RunError::Cancelled { .. } }] if *n == node_id
            ),
            "InvokeError::Cancelled maps to RunError::Cancelled, not Invoke: {:?}",
            stats.node_errors
        );

        Ok(())
    }

    #[test]
    fn impure_node_always_invoked() -> anyhow::Result<()> {
        let graph = test_graph();
        let mut library = test_func_lib(TestFuncHooks::default());

        library.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();

        // Even with cached output, impure node still wants to execute
        execution_graph.set_output_values("get_b", vec![DynamicValue::Static(StaticValue::Int(7))]);
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "Print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn impure_output_stays_resident_for_inspection_after_run() -> anyhow::Result<()> {
        // An impure node re-runs every time, but its output stays resident after a
        // run: outputs are never wiped or evicted, so the editor's on-demand
        // inspector can read the last value even though there's no disk fallback.
        let graph = test_graph();
        let mut library = test_func_lib(default_hooks());
        library.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        let slot = execution_graph.runtime_slot(execution_graph.by_name("get_b").unwrap());
        let outputs = slot
            .output_values()
            .expect("impure node's output stays resident after the run");
        assert_eq!(
            outputs[0].as_f64(),
            Some(11.0),
            "the last-run output is readable for inspection (get_b returns 11)"
        );

        Ok(())
    }
}

mod composite_behavior {
    use super::*;
    use crate::graph::Graph;
    use crate::graph::NodeKind;
    use crate::graph::interface::{GraphId, GraphLink};
    use crate::node::definition::FuncOutput;

    fn func_node(library: &Library, func_name: &str, node_name: &str) -> Node {
        let id = library.by_name(func_name).unwrap().id;
        let mut n = Node::new(NodeKind::Func(id));
        n.name = node_name.to_string();
        n
    }

    fn int_output(name: &str) -> FuncOutput {
        FuncOutput::new(name, DataType::Int)
    }

    /// A graph with no inputs and one output, whose interior is the
    /// impure `get_b` (named `inner_name`) feeding `GraphOutput[0]`.
    fn impure_output_def(library: &Library, name: &str, inner_name: &str) -> Graph {
        let inner = func_node(library, "get_b", inner_name);
        let so = Node::new(NodeKind::GraphOutput);
        let mut graph = Graph::new(name).output(int_output("Out"));
        let inner_id = graph.add(inner);
        let so_id = graph.add(so);
        graph.set_input_binding(InputPort::new(so_id, 0), Binding::bind(inner_id, 0));
        graph
    }

    /// Main graph: one instance of `def` whose output feeds a sink `print`.
    fn main_with(library: &Library, def: Graph) -> Graph {
        main_with_id(library, GraphId::unique(), def)
    }

    fn main_with_id(library: &Library, def_id: GraphId, def: Graph) -> Graph {
        let mut graph = Graph::default();
        graph.insert_graph(def_id, def.clone());
        let inst = graph.add_graph_node(&def, GraphLink::Local(def_id));
        let p = func_node(library, "Print", "p");
        let p_id = graph.add(p);
        graph.set_input_binding(InputPort::new(p_id, 0), Binding::bind(inst, 0));
        graph
    }

    /// `(name in execute_order)` after a second prepare, with a cached
    /// output already present for that node — i.e. "would it re-run?".
    fn reruns_with_cache(graph: &Graph, library: &Library, name: &str) -> bool {
        let mut eg = ExecutionEngine::default();
        eg.update(graph, library).unwrap();
        eg.prepare_execution(true, false, &[]).unwrap();
        assert!(
            execution_node_names_in_order(&eg).contains(&name.to_string()),
            "{name} should run on the first prepare"
        );
        eg.set_output_values(name, vec![DynamicValue::Static(StaticValue::Int(11))]);
        eg.update(graph, library).unwrap();
        eg.prepare_execution(true, false, &[]).unwrap();
        execution_node_names_in_order(&eg).contains(&name.to_string())
    }

    #[test]
    fn composite_reruns_impure_interior() {
        // An impure interior recomputes across a composite boundary like any
        // impure node — flattening must preserve its impurity.
        let mut library = test_func_lib(TestFuncHooks::default());
        library.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;
        let def = impure_output_def(&library, "S", "inner");
        let graph = main_with(&library, def);
        assert!(
            reruns_with_cache(&graph, &library, "inner"),
            "impure interior recomputes through a composite"
        );
    }

    #[test]
    fn update_rejects_func_missing_inside_graph() {
        // The check descends composites: a func only the *interior*
        // references, absent from the lib, is still caught.
        let library = test_func_lib(TestFuncHooks::default());
        let def = impure_output_def(&library, "S", "inner");
        let graph = main_with(&library, def);
        let get_b = library.by_name("get_b").unwrap().id;
        let mut incomplete_library = library.clone();
        incomplete_library.funcs.remove_by_key(&get_b).unwrap();

        // A `Local` def resolves from the graph itself, so validation reaches
        // its interior while every top-level func remains resolvable.
        let mut eg = ExecutionEngine::default();
        let CompileError { message } = eg.update(&graph, &incomplete_library).unwrap_err();
        assert!(
            message.contains(&format!("{get_b:?}")),
            "message should name the interior's missing func, got: {message}"
        );
    }

    #[test]
    fn nested_impure_interior_reruns_when_local_graph_ids_repeat() {
        // A doubly-nested impure node recomputes — flattening preserves its
        // impurity through two composite levels whose map-local ids coincide.
        let mut library = test_func_lib(TestFuncHooks::default());
        library.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;
        let inner_def = impure_output_def(&library, "Inner", "deep");
        let repeated_id = GraphId::unique();
        let mut outer_interior = Graph::new("Outer").output(int_output("Out"));
        outer_interior.insert_graph(repeated_id, inner_def.clone());
        let inner_inst = outer_interior.add_graph_node(&inner_def, GraphLink::Local(repeated_id));
        let so = Node::new(NodeKind::GraphOutput);
        let so_id = outer_interior.add(so);
        outer_interior.set_input_binding(InputPort::new(so_id, 0), Binding::bind(inner_inst, 0));
        let graph = main_with_id(&library, repeated_id, outer_interior);
        assert!(
            reruns_with_cache(&graph, &library, "deep"),
            "doubly-nested impure interior recomputes"
        );
    }

    /// A node seed can target a *graph-interior* node by its authoring id: the seed
    /// resolves through the flatten map (interior flat ids are hashed from the descent
    /// path), runs just that node, and its value reads back under the same authoring id.
    /// The sink `print` (panicking hook) never fires.
    #[tokio::test]
    async fn seeding_a_graph_interior_node_runs_only_it() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let get_b_calls = Arc::new(AtomicUsize::new(0));
        let library = test_func_lib(TestFuncHooks {
            get_b: Arc::new({
                let calls = Arc::clone(&get_b_calls);
                move || {
                    calls.fetch_add(1, Ordering::Relaxed);
                    11
                }
            }),
            ..Default::default()
        });

        // `impure_output_def`'s interior by hand, keeping the interior node's id.
        let inner = func_node(&library, "get_b", "inner");
        let so = Node::new(NodeKind::GraphOutput);
        let mut interior = Graph::new("S").output(int_output("Out"));
        let inner_id = interior.add(inner);
        let so_id = interior.add(so);
        interior.set_input_binding(InputPort::new(so_id, 0), Binding::bind(inner_id, 0));
        let graph = main_with(&library, interior);
        let instance_id = graph
            .iter()
            .find(|node| matches!(node.kind, NodeKind::Graph(_)))
            .unwrap()
            .id;

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        let stats = eg
            .execute_nodes([NodeAddress {
                instances: vec![instance_id],
                node_id: inner_id,
            }])
            .await
            .unwrap();
        assert_eq!(get_b_calls.load(Ordering::Relaxed), 1);
        assert_eq!(stats.executed_nodes.len(), 1);
        assert_eq!(
            eg.get_argument_values(&inner_id).unwrap().outputs[0].as_i64(),
            Some(11),
            "interior value reads back under its authoring id"
        );
    }
}

mod cycle_detection {
    use super::*;

    #[test]
    fn returns_error_with_node_id() {
        let mut graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());

        // Create cycle: sum[0] ← mult (mult already depends on sum)
        let mult_node_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
        bind(&mut graph, "sum", 0, Binding::bind(mult_node_id, 0));

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();

        let err = execution_graph
            .prepare_execution(true, false, &[])
            .expect_err("Expected cycle detection error");
        match err {
            Error::CycleDetected { node_id } => {
                assert_eq!(node_id, mult_node_id);
            }
            _ => panic!("Unexpected error: {err:?}"),
        }
    }
}

mod invalidation {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn clear_resets_graph() -> anyhow::Result<()> {
        let graph = test_graph();
        let library = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert!(!execution_graph.compiled.program.e_nodes.is_empty());

        execution_graph.clear();

        assert!(execution_graph.compiled.program.e_nodes.is_empty());
        assert!(execution_graph.plan.process_order.is_empty());
        // The SoA pools are emptied too (not just the node list).
        assert!(execution_graph.compiled.program.inputs.is_empty());
        assert_eq!(execution_graph.compiled.program.n_outputs(), 0);
        assert!(execution_graph.compiled.program.events.is_empty());

        Ok(())
    }
}

mod execution {
    use super::*;

    #[derive(Debug)]
    struct TestValues {
        a: i64,
        b: i64,
        result: i64,
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn simple_compute() -> anyhow::Result<()> {
        let test_values = Arc::new(Mutex::new(TestValues {
            a: 2,
            b: 5,
            result: 0,
        }));

        let test_values_a = test_values.clone();
        let test_values_b = test_values.clone();
        let test_values_result = test_values.clone();
        let mut library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(test_values_a.try_lock().unwrap().a)),
            get_b: Arc::new(move || test_values_b.try_lock().unwrap().b),
            print: Arc::new(move |result| {
                test_values_result.try_lock().unwrap().result = result;
            }),
        });

        let graph = test_graph();

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;
        // sum = get_a + get_b = 2 + 5 = 7, mult = sum * get_b = 7 * 5 = 35
        assert_eq!(test_values.try_lock()?.result, 35);

        // Changing external state doesn't recompute: get_b is pure, so its digest
        // is stable and the cached value stands.
        test_values.try_lock()?.b = 7;

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;
        assert_eq!(test_values.try_lock()?.result, 35);

        // Make get_b Impure: now it re-reads the value
        library.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;
        // sum = 2 + 7 = 9, mult = 9 * 7 = 63
        assert_eq!(test_values.try_lock()?.result, 63);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn required_none_binding_is_stable() -> anyhow::Result<()> {
        let library = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        // Make sum's first input None (required) — sum and downstream shouldn't execute
        bind(&mut graph, "sum", 0, Binding::None);

        execution_graph.update(&graph, &library).unwrap();

        execution_graph.execute_sinks().await?;
        let order1 = execution_graph.plan.process_order.clone();

        execution_graph.execute_sinks().await?;
        let order2 = execution_graph.plan.process_order.clone();

        // The schedule is deterministic — stable across runs (what actually *runs* can
        // differ as Pure nodes start reusing their cache, but the order can't flap).
        assert_eq!(order1, order2);

        // sum should be marked as missing required inputs
        let sum = execution_graph.by_name("sum").unwrap();
        assert!(execution_graph.node_verdict(sum).missing_required_inputs());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn schedule_stable_across_repeated_runs() -> anyhow::Result<()> {
        let library = test_func_lib(default_hooks());
        let graph = test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        eg.execute_sinks().await?;
        let run1 = execution_node_names_in_order(&eg);
        eg.execute_sinks().await?;
        let run2 = execution_node_names_in_order(&eg);
        eg.execute_sinks().await?;
        let run3 = execution_node_names_in_order(&eg);

        // First run executes everything; once the pure upstream is cached, runs 2
        // and 3 must schedule identically — guards the reused `Scratch` buffers
        // being reset cleanly each run (a missed reset would drift).
        assert_eq!(run2, ["Print"]);
        assert_eq!(run2, run3);
        assert_ne!(run1, run2);

        // The cached product stays correct every run: sum(1+11=12) * get_b(11) = 132.
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
        let vals = eg.get_argument_values(&mult_id).unwrap();
        assert!(matches!(
            vals.outputs[0],
            DynamicValue::Static(StaticValue::Int(132))
        ));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_upstream_output_reused_after_rebinding() -> anyhow::Result<()> {
        let library = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        // Switch mult to const inputs
        bind(&mut graph, "mult", 0, Binding::Const(2.into()));
        bind(&mut graph, "mult", 1, Binding::Const(21.into()));

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "Print"]
        );

        // Switch back to bind from cached get_b — mult re-executes with cached upstream
        let get_b_id = graph
            .find_by_name("get_b", NodeSearch::TopLevel)
            .unwrap()
            .id;
        bind(&mut graph, "mult", 0, Binding::bind(get_b_id, 0));

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "Print"]
        );

        Ok(())
    }

    /// Output buffers are wiped before a re-running node is invoked, so an unwritten
    /// output cannot retain a prior run's value. This sink has no demanded outputs,
    /// therefore leaving one port `Unbound` is valid.
    #[tokio::test(flavor = "multi_thread")]
    async fn unwritten_output_port_is_cleared_before_reexecution() -> anyhow::Result<()> {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crate::async_lambda;
        use crate::graph::Graph;
        use crate::library::Library;
        use crate::node::definition::{Func, FuncOutput};

        let invocations = Arc::new(AtomicUsize::new(0));
        let library: Library = [Func::new(
            "4df6d99f-cb0c-479c-9b94-6549c406d9ab",
            "partial_writer",
        )
        .category("Debug")
        .sink()
        .output(FuncOutput::new("a", DataType::Int))
        .output(FuncOutput::new("b", DataType::Int))
        .lambda(async_lambda!(
            move |_, _, _, _, _, outputs| { invocations = Arc::clone(&invocations) } => {
                let run = invocations.fetch_add(1, Ordering::Relaxed);
                outputs[0] = DynamicValue::Static(StaticValue::Int(100 + run as i64));
                if run == 0 {
                    // Only the first run writes the second port.
                    outputs[1] = DynamicValue::Static(StaticValue::Int(20));
                }
                Ok(())
            }
        ))]
        .into();

        let mut graph = Graph::default();
        let mut node: Node = library.by_name("partial_writer").unwrap().into();
        // Retain the output buffer across runs so the in-place reuse is observable;
        // nodes now default to `CacheMode::None`.
        node.cache = CacheMode::Ram;
        graph.insert("0b35e5e4-be30-4733-a5a2-9d474000de10".into(), node);
        graph.debug_check();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // Run 1: both ports written.
        let stats = eg.execute_sinks().await?;
        assert!(stats.node_errors.is_empty());
        let outputs = eg
            .runtime_slot(eg.by_name("partial_writer").unwrap())
            .output_values()
            .cloned()
            .expect("the node ran, so it holds outputs");
        assert!(
            matches!(outputs[0], DynamicValue::Static(StaticValue::Int(100)))
                && matches!(outputs[1], DynamicValue::Static(StaticValue::Int(20))),
            "run 1 writes both ports: {outputs:?}"
        );

        // Run 2: only port 0 is written. The reused buffer is cleared before invoke,
        // so port 1 cannot masquerade as a value produced by this run.
        let stats = eg.execute_sinks().await?;
        assert!(stats.node_errors.is_empty());
        let outputs = eg
            .runtime_slot(eg.by_name("partial_writer").unwrap())
            .output_values()
            .cloned()
            .expect("the node re-ran (it is impure)");
        assert!(
            matches!(outputs[0], DynamicValue::Static(StaticValue::Int(101))),
            "run 2 rewrites port 0: {outputs:?}"
        );
        assert!(
            matches!(outputs[1], DynamicValue::Unbound),
            "the unwritten port is cleared before invoke: {outputs:?}"
        );

        Ok(())
    }
}

mod node_seeds {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// `test_graph` with every node's cache mode forced to `None`, so any value that
    /// survives a run does so through the node-seed pin, not a retention mode.
    fn uncached_test_graph() -> Graph {
        let mut graph = test_graph();
        for node in graph.nodes.values_mut() {
            node.cache = CacheMode::None;
        }
        graph
    }

    /// Seeding `sum` runs exactly its cone (`get_a`, `get_b`, `sum`) — never the
    /// downstream `mult`/`Print` (the panicking default `print` hook proves it) — and
    /// retains `sum`'s output for read-back despite `CacheMode::None`, while its
    /// equally-uncached upstream is drained as usual.
    #[tokio::test]
    async fn seeded_run_executes_only_the_cone_and_retains_the_output() {
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(1)),
            get_b: Arc::new(|| 11),
            ..Default::default()
        });
        let graph = uncached_test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        let stats = eg.execute_nodes([NodeAddress::root(sum_id)]).await.unwrap();
        assert_eq!(stats.executed_nodes.len(), 3);

        let mut ran = execution_node_names_in_order(&eg);
        ran.sort();
        assert_eq!(ran, ["get_a", "get_b", "sum"], "only sum's cone runs");

        let values = eg.get_argument_values(&sum_id).unwrap();
        assert_eq!(values.outputs[0].as_i64(), Some(12), "1 + 11, retained");

        let get_a_id = graph
            .find_by_name("get_a", NodeSearch::TopLevel)
            .unwrap()
            .id;
        assert!(
            eg.get_argument_values(&get_a_id)
                .unwrap()
                .outputs
                .is_empty(),
            "unpinned None-cache upstream is drained as usual"
        );
    }

    /// A second seeded run of an unchanged graph recomputes nothing: the pinned value
    /// survived eviction, its digest still matches, so the whole cone is a cache hit —
    /// this is what makes repeated previews (and auto-preview) cheap.
    #[tokio::test]
    async fn second_seeded_run_reuses_the_pinned_value() {
        let get_a_calls = Arc::new(AtomicUsize::new(0));
        let get_b_calls = Arc::new(AtomicUsize::new(0));
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new({
                let calls = Arc::clone(&get_a_calls);
                move || {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Ok(1)
                }
            }),
            get_b: Arc::new({
                let calls = Arc::clone(&get_b_calls);
                move || {
                    calls.fetch_add(1, Ordering::Relaxed);
                    11
                }
            }),
            ..Default::default()
        });
        let graph = uncached_test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        eg.execute_nodes([NodeAddress::root(sum_id)]).await.unwrap();
        assert_eq!(get_a_calls.load(Ordering::Relaxed), 1);
        assert_eq!(get_b_calls.load(Ordering::Relaxed), 1);

        let stats = eg.execute_nodes([NodeAddress::root(sum_id)]).await.unwrap();
        assert_eq!(get_a_calls.load(Ordering::Relaxed), 1, "cone not re-run");
        assert_eq!(get_b_calls.load(Ordering::Relaxed), 1, "cone not re-run");
        assert!(stats.executed_nodes.is_empty());
        assert!(stats.cached_nodes.contains(&sum_id), "sum served from RAM");
        assert_eq!(
            eg.get_argument_values(&sum_id).unwrap().outputs[0].as_i64(),
            Some(12),
            "still readable after the reuse run"
        );
    }

    /// Node seeds combine with a sink run: one run drives the sinks (`Print`
    /// fires with 132 = (1+11)*11) *and* pins `sum` — whose value survives its real
    /// consumer's read via the extra virtual consumer — while the unpinned `mult` is
    /// drained by `Print` as usual.
    #[tokio::test]
    async fn node_seed_combines_with_a_sink_run_and_still_retains() {
        let printed: Arc<StdMutex<Vec<i64>>> = Arc::new(StdMutex::new(Vec::new()));
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(1)),
            get_b: Arc::new(|| 11),
            print: Arc::new({
                let printed = Arc::clone(&printed);
                move |v| printed.lock().unwrap().push(v)
            }),
        });
        let graph = uncached_test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        eg.execute(
            RunSeeds {
                sinks: true,
                nodes: vec![NodeAddress::root(sum_id)],
                ..Default::default()
            },
            None,
            CancelToken::never(),
        )
        .await
        .unwrap();

        assert_eq!(*printed.lock().unwrap(), [132], "(1 + 11) * 11");
        assert_eq!(
            eg.get_argument_values(&sum_id).unwrap().outputs[0].as_i64(),
            Some(12),
            "pinned value survives its real consumer's read"
        );
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
        assert!(
            eg.get_argument_values(&mult_id).unwrap().outputs.is_empty(),
            "unpinned None-cache node is drained by its consumer"
        );
    }

    /// A seed that doesn't resolve against the compiled program (deleted or disabled
    /// node, stale id) fails the run — seeds are batched with the graph they target,
    /// so a miss is inconsistent caller state, not something to silently skip. The
    /// panicking default hooks prove no lambda fires.
    #[tokio::test]
    async fn unresolvable_node_seed_fails_the_run() {
        let library = test_func_lib(TestFuncHooks::default());
        let graph = test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        let bogus: NodeId = NodeId::from_u128(0xdead_beef);
        let address = NodeAddress::root(bogus);
        let err = eg.execute_nodes([address.clone()]).await.unwrap_err();
        assert!(matches!(err, Error::NodeSeedNotFound { address: missing } if missing == address));
    }
}

mod argument_values {
    use super::*;

    #[test]
    fn nonexistent_node_returns_none() {
        let graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &library).unwrap();

        let nonexistent_id: NodeId = "00000000-0000-0000-0000-000000000000".into();
        assert!(
            execution_graph
                .get_argument_values(&nonexistent_id)
                .is_none()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_const_bindings() -> anyhow::Result<()> {
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || unreachable!()),
            get_b: Arc::new(move || unreachable!()),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(3)));
        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(5)));
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        let values = execution_graph.get_argument_values(&mult_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(matches!(
            values.inputs[0],
            Some(DynamicValue::Static(StaticValue::Int(3)))
        ));
        assert!(matches!(
            values.inputs[1],
            Some(DynamicValue::Static(StaticValue::Int(5)))
        ));

        // 3 * 5 = 15
        assert_eq!(values.outputs.len(), 1);
        assert!(matches!(
            values.outputs[0],
            DynamicValue::Static(StaticValue::Int(15))
        ));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_bound_outputs() -> anyhow::Result<()> {
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(2)),
            get_b: Arc::new(move || 5),
            print: Arc::new(move |_| {}),
        });

        let graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        // sum: inputs are get_a(2.0) and get_b(5.0), output is 2+5=7
        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        let values = execution_graph.get_argument_values(&sum_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(
            matches!(values.inputs[0], Some(DynamicValue::Static(StaticValue::Float(v))) if v.approximately_eq(2.0))
        );
        assert!(
            matches!(values.inputs[1], Some(DynamicValue::Static(StaticValue::Float(v))) if v.approximately_eq(5.0))
        );
        assert_eq!(values.outputs.len(), 1);
        assert!(matches!(
            values.outputs[0],
            DynamicValue::Static(StaticValue::Int(7))
        ));

        // mult: inputs are sum(7) and get_b(5.0), output is 7*5=35
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;
        let values = execution_graph.get_argument_values(&mult_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(matches!(
            values.inputs[0],
            Some(DynamicValue::Static(StaticValue::Int(7)))
        ));
        assert!(
            matches!(values.inputs[1], Some(DynamicValue::Static(StaticValue::Float(v))) if v.approximately_eq(5.0))
        );
        assert_eq!(values.outputs.len(), 1);
        assert!(matches!(
            values.outputs[0],
            DynamicValue::Static(StaticValue::Int(35))
        ));

        // print: input is mult(35), no outputs
        let print_id = graph
            .find_by_name("Print", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let values = execution_graph.get_argument_values(&print_id).unwrap();

        assert_eq!(values.inputs.len(), 1);
        assert!(matches!(
            values.inputs[0],
            Some(DynamicValue::Static(StaticValue::Int(35)))
        ));
        assert!(values.outputs.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_none_binding() -> anyhow::Result<()> {
        let mut library = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        library.by_name_mut("mult").unwrap().inputs[1].required = false;
        bind(&mut graph, "mult", 1, Binding::None);
        let mult_id = graph.find_by_name("mult", NodeSearch::TopLevel).unwrap().id;

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_sinks().await?;

        let values = execution_graph.get_argument_values(&mult_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(values.inputs[0].is_some());
        // None binding returns None value
        assert!(values.inputs[1].is_none());

        Ok(())
    }

    #[test]
    fn before_execution() -> anyhow::Result<()> {
        let graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &library).unwrap();

        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        let values = execution_graph.get_argument_values(&sum_id).unwrap();

        // Before execution: all inputs are None (no upstream values yet)
        assert_eq!(values.inputs.len(), 2);
        assert!(values.inputs[0].is_none());
        assert!(values.inputs[1].is_none());
        assert!(values.outputs.is_empty());

        Ok(())
    }
}

mod error_propagation {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn node_error_propagates_to_dependents() -> anyhow::Result<()> {
        let graph = test_graph();
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Err(anyhow::anyhow!("Intentional failure in get_a"))),
            get_b: Arc::new(|| 42),
            print: Arc::new(|_| {}),
        });

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();

        let stats = execution_graph.execute_sinks().await?;

        // Errors are reported through the run stats (the per-run channel), not the
        // cross-run cache; the cache only reflects which outputs survived.
        let error_for = |name: &str| {
            let id = execution_graph.by_name(name).unwrap().id;
            stats.node_errors.iter().find(move |e| e.node_id == id)
        };
        let output_values = |name: &str| {
            execution_graph
                .runtime_slot(execution_graph.by_name(name).unwrap())
                .output_values()
                .cloned()
        };

        // get_a fails with error, no outputs.
        assert!(
            error_for("get_a")
                .unwrap()
                .error
                .to_string()
                .contains("Intentional failure")
        );
        assert!(output_values("get_a").is_none());

        // get_b succeeds: no error, output present.
        assert!(error_for("get_b").is_none());
        let get_b_out = output_values("get_b").unwrap();
        assert!(get_b_out[0].as_f64().unwrap().approximately_eq(42.0));

        // sum depends on get_a, mult on sum, print on mult — each inherits the
        // upstream error and drops its output.
        for name in ["sum", "mult", "Print"] {
            assert!(
                error_for(name)
                    .unwrap_or_else(|| panic!("{name} should carry an upstream error"))
                    .error
                    .to_string()
                    .contains("upstream"),
                "{name} should report an upstream error",
            );
            assert!(
                output_values(name).is_none(),
                "{name} should have no output"
            );
        }

        // 4 errors total: get_a original + 3 upstream-propagated.
        assert_eq!(stats.node_errors.len(), 4);

        Ok(())
    }
}

mod stats {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_inputs_reported() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let library = test_func_lib(default_hooks());

        // Remove sum's first input (required)
        bind(&mut graph, "sum", 0, Binding::None);

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        let stats = execution_graph.execute_sinks().await?;

        // sum[0] should appear in missing_inputs
        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        assert!(
            stats
                .missing_inputs
                .iter()
                .any(|p| p.node_id == sum_id && p.port_idx == 0),
            "Expected sum input 0 in missing_inputs, got: {:?}",
            stats.missing_inputs
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn executed_nodes_reported() -> anyhow::Result<()> {
        let graph = test_graph();
        let library = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        let stats = execution_graph.execute_sinks().await?;

        // All 5 nodes should be reported as executed
        assert_eq!(stats.executed_nodes.len(), 5);

        // Each node should have a non-negative elapsed time
        for node_stats in &stats.executed_nodes {
            assert!(
                node_stats.elapsed_secs >= 0.0,
                "node {:?} has negative elapsed_secs",
                node_stats.node_id
            );
        }

        // Verify specific node IDs are present
        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        let print_id = graph
            .find_by_name("Print", NodeSearch::TopLevel)
            .unwrap()
            .id;
        assert!(stats.executed_nodes.iter().any(|n| n.node_id == sum_id));
        assert!(stats.executed_nodes.iter().any(|n| n.node_id == print_id));

        // No errors on first clean run
        assert!(stats.node_errors.is_empty());
        assert!(stats.missing_inputs.is_empty());

        Ok(())
    }
}

mod events {
    use super::*;
    use crate::async_lambda;
    use crate::execution::event::EventRef;
    use crate::node::definition::{Func, FuncInput, FuncOutput};
    use crate::node::event::EventLambda;

    const EMIT_FUNC: FuncId = FuncId::from_u128(0xE311);
    const RECV_FUNC: FuncId = FuncId::from_u128(0xE322);

    struct EventFixture {
        library: Library,
        graph: Graph,
        emit_id: NodeId,
        emit_calls: Arc<Mutex<i64>>,
        recv_values: Arc<Mutex<Vec<i64>>>,
    }

    // `emit`: impure source with output 0 and one event ("tick") subscribed to
    // by `recv`. `recv`: impure sink bound to emit's output. Neither is a
    // sink, so only event-driven execution reaches them.
    fn build() -> EventFixture {
        let emit_calls = Arc::new(Mutex::new(0));
        let recv_values = Arc::new(Mutex::new(Vec::new()));
        let emit_calls_l = emit_calls.clone();
        let recv_values_l = recv_values.clone();

        // Both funcs are Impure non-sinks (the `Func::new` default).
        let mut library = Library::default();
        library.add(
            Func::new(EMIT_FUNC, "emit")
                .output(FuncOutput::new("out", DataType::Int))
                .event("tick", EventLambda::new(|_state| Box::pin(async move {})))
                .lambda(async_lambda!(
                    move |_, _, _, _, _, outputs| { calls = emit_calls_l.clone() } => {
                        let mut n = calls.lock().await;
                        *n += 1;
                        outputs[0] = DynamicValue::Static(StaticValue::Int(*n));
                        Ok(())
                    }
                )),
        );
        library.add(
            Func::new(RECV_FUNC, "recv")
                .input(FuncInput::required("in", DataType::Int))
                .lambda(async_lambda!(
                    move |_, _, _, inputs, _, _| { values = recv_values_l.clone() } => {
                        values.lock().await.push(inputs[0].value.as_i64().unwrap());
                        Ok(())
                    }
                )),
        );

        let emit_id = NodeId::unique();
        let recv_id = NodeId::unique();

        let mut graph = Graph::default();
        graph.insert(emit_id, node(&library, "emit"));
        graph.insert(recv_id, node(&library, "recv"));
        graph.subscribe(emit_id, 0, recv_id);
        graph.set_input_binding(InputPort::new(recv_id, 0), Binding::bind(emit_id, 0));
        graph.debug_check();

        EventFixture {
            library,
            graph,
            emit_id,
            emit_calls,
            recv_values,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn execute_events_runs_subscribers() -> anyhow::Result<()> {
        let f = build();
        let mut eg = ExecutionEngine::default();
        eg.update(&f.graph, &f.library).unwrap();

        let stats = eg
            .execute_events([EventRef {
                node_id: f.emit_id,
                event_idx: 0,
            }])
            .await?;

        // recv subscribes to emit's tick → recv is the root, emit runs as its dep
        assert_eq!(execution_node_names_in_order(&eg), ["emit", "recv"]);
        assert_eq!(*f.emit_calls.lock().await, 1);
        assert_eq!(*f.recv_values.lock().await, vec![1]);

        // The triggering event is echoed back in the stats
        assert_eq!(stats.triggered_events.len(), 1);
        assert_eq!(stats.triggered_events[0].node_id, f.emit_id);
        assert_eq!(stats.triggered_events[0].event_idx, 0);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_triggers_collects_nodes_with_subscribers() -> anyhow::Result<()> {
        let f = build();
        let mut eg = ExecutionEngine::default();
        eg.update(&f.graph, &f.library).unwrap();

        // sinks=false, event_triggers=true → emit (owns a subscribed event)
        // becomes a root; recv is downstream of emit, not a root.
        eg.execute(
            RunSeeds {
                event_triggers: true,
                ..Default::default()
            },
            None,
            CancelToken::never(),
        )
        .await?;

        assert_eq!(execution_node_names_in_order(&eg), ["emit"]);
        assert_eq!(*f.emit_calls.lock().await, 1);
        assert!(f.recv_values.lock().await.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn active_event_triggers_lists_live_events() -> anyhow::Result<()> {
        let f = build();
        let mut eg = ExecutionEngine::default();
        eg.update(&f.graph, &f.library).unwrap();

        let stats = eg
            .execute(
                RunSeeds {
                    event_triggers: true,
                    ..Default::default()
                },
                None,
                CancelToken::never(),
            )
            .await?;
        let triggers = eg.active_event_triggers(&stats);

        // emit executed and has a populated lambda + a subscriber → one trigger.
        // recv has no events → contributes nothing.
        assert_eq!(triggers.len(), 1);
        assert_eq!(triggers[0].event.node_id, f.emit_id);
        assert_eq!(triggers[0].event.event_idx, 0);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn active_event_triggers_empty_without_subscribers() -> anyhow::Result<()> {
        let mut f = build();
        // Drop the subscriber but keep emit reachable by making it a sink.
        let emit_id = f.emit_id;
        let recv_id = f
            .graph
            .find_by_name("recv", NodeSearch::TopLevel)
            .unwrap()
            .id;
        f.graph.unsubscribe(emit_id, 0, recv_id);
        f.library.by_name_mut("emit").unwrap().sink = true;

        let mut eg = ExecutionEngine::default();
        eg.update(&f.graph, &f.library).unwrap();
        let stats = eg.execute_sinks().await?;

        // emit ran, but its event has no subscribers → no live triggers.
        assert!(stats.executed_nodes.iter().any(|n| n.node_id == f.emit_id));
        assert!(eg.active_event_triggers(&stats).is_empty());

        Ok(())
    }

    const SOURCE_FUNC: FuncId = FuncId::from_u128(0xE401);
    const SINK_FUNC: FuncId = FuncId::from_u128(0xE402);

    /// A `RunSinks` special node subscribed to an event fires no cone of its own
    /// (it has no ports) — instead firing that event runs *every* sink, exactly as
    /// pressing "Run" would. Here `emit`'s tick reaches only the `RunSinks` sink, yet
    /// the independent `source → sink` cone runs, while `emit` (not a sink,
    /// not in that cone) does not.
    #[tokio::test(flavor = "multi_thread")]
    async fn run_sinks_node_runs_all_sinks_on_event() -> anyhow::Result<()> {
        use crate::graph::NodeKind;
        use crate::node::special::SpecialNode;

        let source_calls = Arc::new(Mutex::new(0i64));
        let sink_values = Arc::new(Mutex::new(Vec::<i64>::new()));
        let source_l = source_calls.clone();
        let sink_l = sink_values.clone();

        let mut library = Library::default();
        // An impure emitter carrying a "tick" event but no data wiring of its own.
        library.add(
            Func::new(EMIT_FUNC, "emit")
                .output(FuncOutput::new("out", DataType::Int))
                .event("tick", EventLambda::new(|_state| Box::pin(async move {})))
                .lambda(async_lambda!(move |_, _, _, _, _, outputs| {
                    outputs[0] = DynamicValue::Static(StaticValue::Int(0));
                    Ok(())
                })),
        );
        // An impure source feeding the sink — proves the sink's whole cone runs.
        library.add(
            Func::new(SOURCE_FUNC, "source")
                .output(FuncOutput::new("out", DataType::Int))
                .lambda(async_lambda!(
                    move |_, _, _, _, _, outputs| { calls = source_l.clone() } => {
                        let mut n = calls.lock().await;
                        *n += 1;
                        outputs[0] = DynamicValue::Static(StaticValue::Int(*n));
                        Ok(())
                    }
                )),
        );
        // An impure sink recording each value it receives.
        library.add(
            Func::new(SINK_FUNC, "sink")
                .sink()
                .input(FuncInput::required("in", DataType::Int))
                .lambda(async_lambda!(
                    move |_, _, _, inputs, _, _| { values = sink_l.clone() } => {
                        values.lock().await.push(inputs[0].value.as_i64().unwrap());
                        Ok(())
                    }
                )),
        );

        let emit_id = NodeId::unique();
        let source_id = NodeId::unique();
        let sink_id = NodeId::unique();
        let trigger_id = NodeId::unique();

        let mut graph = Graph::default();
        graph.insert(emit_id, node(&library, "emit"));
        graph.insert(source_id, node(&library, "source"));
        graph.insert(sink_id, node(&library, "sink"));
        // The RunSinks sink — no ports; subscribes to emit's tick.
        let mut trigger = Node::new(NodeKind::Special(SpecialNode::RunSinks));
        trigger.name = "trigger".to_string();
        graph.insert(trigger_id, trigger);

        // The sink's cone (source → sink) is wholly independent of emit.
        graph.set_input_binding(InputPort::new(sink_id, 0), Binding::bind(source_id, 0));
        graph.subscribe(emit_id, 0, trigger_id);
        graph.debug_check_for_execution(&library);

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library)?;

        let stats = eg
            .execute_events([EventRef {
                node_id: emit_id,
                event_idx: 0,
            }])
            .await?;

        // The sink cone ran; emit (neither a sink nor in that cone) did not.
        let ran = execution_node_names_in_order(&eg);
        assert!(ran.contains(&"source".to_string()), "ran = {ran:?}");
        assert!(ran.contains(&"sink".to_string()), "ran = {ran:?}");
        assert!(!ran.contains(&"emit".to_string()), "ran = {ran:?}");
        assert_eq!(*source_calls.lock().await, 1);
        assert_eq!(*sink_values.lock().await, vec![1]);
        assert_eq!(stats.triggered_events.len(), 1);

        // The RunSinks sink is itself a sink, so it runs (its no-op lambda)
        // alongside the promoted sinks — never seeded as a plain subscriber cone.
        assert!(ran.contains(&"trigger".to_string()), "ran = {ran:?}");
        assert!(
            eg.plan.process_order.contains(&trigger_id),
            "the RunSinks sink runs as a sink"
        );

        Ok(())
    }

    /// Without the `RunSinks` sink, firing `emit`'s tick reaches no subscriber, so
    /// the same sink cone is left untouched — isolating the sink as the cause.
    #[tokio::test(flavor = "multi_thread")]
    async fn event_without_run_sinks_sink_runs_nothing() -> anyhow::Result<()> {
        let source_calls = Arc::new(Mutex::new(0i64));
        let source_l = source_calls.clone();

        let mut library = Library::default();
        library.add(
            Func::new(EMIT_FUNC, "emit")
                .event("tick", EventLambda::new(|_state| Box::pin(async move {})))
                .lambda(async_lambda!(move |_, _, _, _, _, _| { Ok(()) })),
        );
        library.add(
            Func::new(SOURCE_FUNC, "source")
                .output(FuncOutput::new("out", DataType::Int))
                .lambda(async_lambda!(
                    move |_, _, _, _, _, outputs| { calls = source_l.clone() } => {
                        *calls.lock().await += 1;
                        outputs[0] = DynamicValue::Static(StaticValue::Int(1));
                        Ok(())
                    }
                )),
        );
        library.add(
            Func::new(SINK_FUNC, "sink")
                .sink()
                .input(FuncInput::required("in", DataType::Int))
                .lambda(async_lambda!(move |_, _, _, _, _, _| { Ok(()) })),
        );

        let emit_id = NodeId::unique();
        let source_id = NodeId::unique();
        let sink_id = NodeId::unique();

        let mut graph = Graph::default();
        graph.insert(emit_id, node(&library, "emit"));
        graph.insert(source_id, node(&library, "source"));
        graph.insert(sink_id, node(&library, "sink"));
        graph.set_input_binding(InputPort::new(sink_id, 0), Binding::bind(source_id, 0));
        graph.debug_check_for_execution(&library);

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library)?;
        eg.execute_events([EventRef {
            node_id: emit_id,
            event_idx: 0,
        }])
        .await?;

        assert!(execution_node_names_in_order(&eg).is_empty());
        assert_eq!(*source_calls.lock().await, 0);

        Ok(())
    }
}

mod output_demand {
    use super::*;
    use crate::async_lambda;
    use crate::node::definition::{Func, FuncInput, FuncOutput};
    use crate::node::lambda::OutputDemand;

    const SPLIT_FUNC: FuncId = FuncId::from_u128(0x5911);
    const SINK_FUNC: FuncId = FuncId::from_u128(0x5922);

    #[tokio::test(flavor = "multi_thread")]
    async fn unused_output_marked_skip() -> anyhow::Result<()> {
        let seen_demand: Arc<Mutex<Vec<OutputDemand>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_demand_l = seen_demand.clone();

        let mut library = Library::default();
        library.add(
            Func::new(SPLIT_FUNC, "split")
                .output(FuncOutput::new("a", DataType::Int))
                .output(FuncOutput::new("b", DataType::Int))
                .lambda(async_lambda!(
                    move |_, _, _, _, demand, outputs| { seen = seen_demand_l.clone() } => {
                        seen.lock().await.extend_from_slice(demand);
                        outputs[0] = DynamicValue::Static(StaticValue::Int(1));
                        outputs[1] = DynamicValue::Static(StaticValue::Int(2));
                        Ok(())
                    }
                )),
        );
        library.add(
            Func::new(SINK_FUNC, "sink")
                .sink()
                .input(FuncInput::required("in", DataType::Int))
                .lambda(async_lambda!(|_, _, _, _, _, _| { Ok(()) })),
        );

        let split_id = NodeId::unique();
        let sink_id = NodeId::unique();
        let mut graph = Graph::default();
        graph.insert(split_id, node(&library, "split"));
        graph.insert(sink_id, node(&library, "sink"));
        // Consume only output 0; output 1 has no consumer.
        graph.set_input_binding(InputPort::new(sink_id, 0), Binding::bind(split_id, 0));
        graph.debug_check();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_sinks().await?;

        let split = eg.by_name("split").unwrap();
        assert_eq!(eg.node_output_demand(split)[0], OutputDemand::Produce);
        assert_eq!(eg.node_output_demand(split)[1], OutputDemand::Skip);
        assert_eq!(eg.node_output_readers(split), &[1, 0]);

        // The lambda observed Produce for the consumed output, Skip for the other.
        assert_eq!(
            *seen_demand.lock().await,
            vec![OutputDemand::Produce, OutputDemand::Skip]
        );

        Ok(())
    }

    /// A pinned output (e.g. a GUI inspector reading a port live) makes an
    /// otherwise-unconsumed output `Produce` too — same "split" fixture as
    /// `unused_output_marked_skip`, output 1 still has no in-graph consumer, but
    /// is now flagged pinned.
    #[tokio::test(flavor = "multi_thread")]
    async fn pinned_output_is_needed_with_no_consumer() -> anyhow::Result<()> {
        let seen_demand: Arc<Mutex<Vec<OutputDemand>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_demand_l = seen_demand.clone();

        let mut library = Library::default();
        library.add(
            Func::new(SPLIT_FUNC, "split")
                .output(FuncOutput::new("a", DataType::Int))
                .output(FuncOutput::new("b", DataType::Int))
                .lambda(async_lambda!(
                    move |_, _, _, _, demand, outputs| { seen = seen_demand_l.clone() } => {
                        seen.lock().await.extend_from_slice(demand);
                        outputs[0] = DynamicValue::Static(StaticValue::Int(1));
                        outputs[1] = DynamicValue::Static(StaticValue::Int(2));
                        Ok(())
                    }
                )),
        );
        library.add(
            Func::new(SINK_FUNC, "sink")
                .sink()
                .input(FuncInput::required("in", DataType::Int))
                .lambda(async_lambda!(|_, _, _, _, _, _| { Ok(()) })),
        );

        let split_id = NodeId::unique();
        let sink_id = NodeId::unique();
        let mut graph = Graph::default();
        graph.insert(split_id, node(&library, "split"));
        graph.insert(sink_id, node(&library, "sink"));
        // Output 0 has a real consumer; output 1 has none, but is pinned.
        graph.set_input_binding(InputPort::new(sink_id, 0), Binding::bind(split_id, 0));
        graph.set_output_pinned(OutputPort::new(split_id, 1), true);
        graph.debug_check();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_sinks().await?;

        let split = eg.by_name("split").unwrap();
        assert_eq!(eg.node_output_demand(split)[0], OutputDemand::Produce);
        assert_eq!(
            eg.node_output_demand(split)[1],
            OutputDemand::Produce,
            "the planner demands a pinned port even with no in-graph consumer"
        );
        assert_eq!(
            eg.node_output_readers(split),
            &[1, 0],
            "the pinned output does not create a synthetic binding reader"
        );

        // The lambda computes both outputs instead of skipping the unconsumed one.
        assert_eq!(
            *seen_demand.lock().await,
            vec![OutputDemand::Produce, OutputDemand::Produce]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_node_reruns_when_a_previously_skipped_output_becomes_needed()
    -> anyhow::Result<()> {
        let split_calls = Arc::new(Mutex::new(0));
        let received = Arc::new(Mutex::new(Vec::new()));
        let split_calls_l = split_calls.clone();
        let received_l = received.clone();

        let mut library = Library::default();
        library.add(
            Func::new(SPLIT_FUNC, "split")
                .pure()
                .output(FuncOutput::new("a", DataType::Int))
                .output(FuncOutput::new("b", DataType::Int))
                .lambda(async_lambda!(
                    move |_, _, _, _, demand, outputs| { calls = split_calls_l.clone() } => {
                        *calls.lock().await += 1;
                        if !demand[0].is_skip() {
                            outputs[0] = StaticValue::Int(10).into();
                        }
                        if !demand[1].is_skip() {
                            outputs[1] = StaticValue::Int(20).into();
                        }
                        Ok(())
                    }
                )),
        );
        library.add(
            Func::new(SINK_FUNC, "sink")
                .sink()
                .input(FuncInput::required("in", DataType::Int))
                .lambda(async_lambda!(
                    move |_, _, _, inputs, _, _| { received = received_l.clone() } => {
                        received.lock().await.push(inputs[0].value.as_i64().unwrap());
                        Ok(())
                    }
                )),
        );

        let split_id = NodeId::unique();
        let sink_a_id = NodeId::unique();
        let sink_b_id = NodeId::unique();
        let mut split = node(&library, "split");
        split.cache = CacheMode::Ram;
        let mut graph = Graph::default();
        graph.insert(split_id, split);
        graph.insert(sink_a_id, node(&library, "sink"));
        graph.set_input_binding(InputPort::new(sink_a_id, 0), Binding::bind(split_id, 0));

        let mut engine = ExecutionEngine::default();
        engine.update(&graph, &library)?;
        engine.execute_sinks().await?;

        graph.insert(sink_b_id, node(&library, "sink"));
        graph.set_input_binding(InputPort::new(sink_b_id, 0), Binding::bind(split_id, 1));
        engine.update(&graph, &library)?;
        engine.execute_sinks().await?;

        assert_eq!(*split_calls.lock().await, 2);
        let mut received = received.lock().await.clone();
        received.sort_unstable();
        assert_eq!(received, vec![10, 10, 20]);
        Ok(())
    }
}

mod topology {
    use super::*;
    use common::FloatExt;

    #[tokio::test(flavor = "multi_thread")]
    async fn removing_node_rebuilds_id_keyed_edges() -> anyhow::Result<()> {
        let printed = Arc::new(Mutex::new(0i64));
        let printed_l = printed.clone();
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 5),
            print: Arc::new(move |v| *printed_l.try_lock().unwrap() = v),
        });

        let mut graph = test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        assert_eq!(eg.compiled.program.e_nodes.len(), 5);

        // Remove get_b — a middle node feeding sum[1] and mult[1] (both optional).
        // The surviving direct-ID bindings remain valid.
        let get_b_id = graph
            .find_by_name("get_b", NodeSearch::TopLevel)
            .unwrap()
            .id;
        graph.detach_node(get_b_id);
        graph.debug_check();

        eg.update(&graph, &library).unwrap();
        assert_eq!(eg.compiled.program.e_nodes.len(), 4);
        assert!(eg.by_name("get_b").is_none());

        eg.execute_sinks().await?;

        // sum = get_a(2) + none(0) = 2; mult = sum(2) * none(default 1) = 2
        assert_eq!(*printed.lock().await, 2);

        // sum's Bind to get_a still resolves after the index remap.
        let sum_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        let vals = eg.get_argument_values(&sum_id).unwrap();
        assert!(
            matches!(vals.inputs[0], Some(DynamicValue::Static(StaticValue::Float(v))) if v.approximately_eq(2.0))
        );
        assert!(vals.inputs[1].is_none());
        assert!(matches!(
            vals.outputs[0],
            DynamicValue::Static(StaticValue::Int(2))
        ));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn empty_graph_executes_cleanly() -> anyhow::Result<()> {
        let graph = Graph::default();
        let library = Library::default();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        assert!(eg.is_empty());

        let stats = eg.execute_sinks().await?;
        assert!(stats.executed_nodes.is_empty());
        assert!(stats.node_errors.is_empty());
        assert!(stats.missing_inputs.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multiple_sinks_all_execute() -> anyhow::Result<()> {
        let printed = Arc::new(Mutex::new(Vec::<i64>::new()));
        let printed_l = printed.clone();
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 5),
            print: Arc::new(move |v| printed_l.try_lock().unwrap().push(v)),
        });

        // Two independent sink chains: get_a→print1, get_b→print2.
        let get_a_id = NodeId::unique();
        let get_b_id = NodeId::unique();
        let print1_id = NodeId::unique();
        let print2_id = NodeId::unique();

        let mut graph = Graph::default();
        graph.insert(get_a_id, node(&library, "get_a"));
        graph.insert(get_b_id, node(&library, "get_b"));
        graph.insert(print1_id, node(&library, "Print"));
        graph.insert(print2_id, node(&library, "Print"));
        graph.set_input_binding(InputPort::new(print1_id, 0), Binding::bind(get_a_id, 0));
        graph.set_input_binding(InputPort::new(print2_id, 0), Binding::bind(get_b_id, 0));
        graph.debug_check();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        let stats = eg.execute_sinks().await?;

        // Both sinks plus both sources execute exactly once.
        assert_eq!(stats.executed_nodes.len(), 4);
        let mut got = printed.lock().await.clone();
        got.sort();
        assert_eq!(got, vec![2, 5]);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_output_survives_node_removal() -> anyhow::Result<()> {
        // Both sources are Pure, so their outputs are cached across runs.
        // Removing one chain must preserve the survivor's ID-keyed slot.
        let calls_a = Arc::new(Mutex::new(0));
        let calls_a_l = calls_a.clone();
        let calls_b = Arc::new(Mutex::new(0));
        let calls_b_l = calls_b.clone();
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || {
                *calls_a_l.try_lock().unwrap() += 1;
                Ok(2)
            }),
            get_b: Arc::new(move || {
                *calls_b_l.try_lock().unwrap() += 1;
                5
            }),
            print: Arc::new(|_| {}),
        });

        let get_b_id = NodeId::unique();
        let print_b_id = NodeId::unique();
        let get_a_id = NodeId::unique();
        let print_a_id = NodeId::unique();

        let mut graph = Graph::default();
        graph.insert(get_b_id, node(&library, "get_b"));
        graph.insert(print_b_id, node(&library, "Print"));
        graph.insert(get_a_id, node(&library, "get_a"));
        graph.insert(print_a_id, node(&library, "Print"));
        graph.set_input_binding(InputPort::new(print_b_id, 0), Binding::bind(get_b_id, 0));
        graph.set_input_binding(InputPort::new(print_a_id, 0), Binding::bind(get_a_id, 0));
        graph.debug_check();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_sinks().await?;
        assert_eq!(*calls_a.lock().await, 1);
        assert_eq!(*calls_b.lock().await, 1);

        let survivor_id = get_a_id;
        let expected_value = 2.0;
        let survivor_calls = &calls_a;

        graph.detach_node(get_b_id);
        graph.detach_node(print_b_id);
        graph.debug_check();

        eg.update(&graph, &library).unwrap();

        let stats = eg.execute_sinks().await?;

        assert_eq!(
            *survivor_calls.lock().await,
            1,
            "survivor recomputed after unrelated node removal"
        );
        assert!(stats.cached_nodes.contains(&survivor_id));
        let vals = eg.get_argument_values(&survivor_id).unwrap();
        assert!(
            matches!(vals.outputs[0], DynamicValue::Static(StaticValue::Float(v)) if v.approximately_eq(expected_value))
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn repeated_structural_churn_stays_correct() -> anyhow::Result<()> {
        // Grow→shrink the graph repeatedly on ONE ExecutionEngine, re-executing
        // each step. Stresses the SoA pool and ID-keyed node-map rebuild across many
        // updates (pools grow 2→4 then shrink 4→2 each round).
        let printed = Arc::new(Mutex::new(Vec::<i64>::new()));
        let p = printed.clone();
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 5),
            print: Arc::new(move |v| p.try_lock().unwrap().push(v)),
        });

        let get_a_id = NodeId::unique();
        let print_a_id = NodeId::unique();
        let mut graph = Graph::default();
        graph.insert(get_a_id, node(&library, "get_a"));
        graph.insert(print_a_id, node(&library, "Print"));
        graph.set_input_binding(InputPort::new(print_a_id, 0), Binding::bind(get_a_id, 0));
        graph.debug_check();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_sinks().await?;

        for round in 0..3 {
            // Add a get_b → print chain.
            let gb = NodeId::unique();
            let pb = NodeId::unique();
            graph.insert(gb, node(&library, "get_b"));
            graph.insert(pb, node(&library, "Print"));
            graph.set_input_binding(InputPort::new(pb, 0), Binding::bind(gb, 0));
            graph.debug_check();
            eg.update(&graph, &library).unwrap();
            assert_eq!(eg.compiled.program.e_nodes.len(), 4, "round {round} grow");
            printed.lock().await.clear();
            eg.execute_sinks().await?;
            let mut got = printed.lock().await.clone();
            got.sort();
            assert_eq!(got, vec![2, 5], "round {round} grow values");

            // Remove it again.
            graph.detach_node(gb);
            graph.detach_node(pb);
            graph.debug_check();
            eg.update(&graph, &library).unwrap();
            assert_eq!(eg.compiled.program.e_nodes.len(), 2, "round {round} shrink");
            printed.lock().await.clear();
            eg.execute_sinks().await?;
            assert_eq!(
                *printed.lock().await,
                vec![2],
                "round {round} shrink values"
            );
        }

        Ok(())
    }
}

mod graph {
    use super::*;
    use crate::graph::Graph;
    use crate::graph::NodeKind;
    use crate::graph::interface::{GraphEvent, GraphId, GraphLink};
    use crate::node::definition::{Func, FuncId, FuncInput, FuncOutput};
    use crate::node::event::EventLambda;
    use std::sync::Mutex as StdMutex;

    fn fnode(library: &Library, name: &str) -> Node {
        library.by_name(name).unwrap().into()
    }

    fn int_out(name: &str) -> FuncOutput {
        FuncOutput::new(name, DataType::Int)
    }

    /// `in(A,B) -> sum -> out(Sum)`.
    fn wrap_sum_def(library: &Library) -> Graph {
        let in_node = Node::new(NodeKind::GraphInput);
        let sum = fnode(library, "sum");
        let out = Node::new(NodeKind::GraphOutput);

        let mut graph = Graph::new("WrapSum")
            .category("Test")
            .input(FuncInput::required("A", DataType::Int))
            .input(FuncInput::optional("B", DataType::Int))
            .output(int_out("Sum"));
        let in_id = graph.add(in_node);
        let sum_id = graph.add(sum);
        let out_id = graph.add(out);
        graph.set_input_binding(InputPort::new(sum_id, 0), Binding::bind(in_id, 0));
        graph.set_input_binding(InputPort::new(sum_id, 1), Binding::bind(in_id, 1));
        graph.set_input_binding(InputPort::new(out_id, 0), Binding::bind(sum_id, 0));

        graph
    }

    fn local_instance(parent: &mut Graph, graph: Graph) -> Node {
        let graph_id = GraphId::unique();
        let node = Node::graph_instance(&graph, GraphLink::Local(graph_id));
        parent.insert_graph(graph_id, graph);
        node
    }

    /// A composite computes through the flattened interior end to end.
    #[tokio::test(flavor = "multi_thread")]
    async fn composite_computes_via_flattening() -> anyhow::Result<()> {
        let captured = Arc::new(StdMutex::new(Vec::<i64>::new()));
        let hooks = TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 4),
            print: {
                let c = captured.clone();
                Arc::new(move |v| c.lock().unwrap().push(v))
            },
        };
        let library = test_func_lib(hooks);
        let def = wrap_sum_def(&library);

        let get_a = fnode(&library, "get_a");
        let get_b = fnode(&library, "get_b");
        let print = fnode(&library, "Print");

        let mut graph = Graph::default();
        let c = local_instance(&mut graph, def);
        let a_id = graph.add(get_a);
        let b_id = graph.add(get_b);
        let c_id = graph.add(c);
        let print_id = graph.add(print);
        graph.set_input_binding(InputPort::new(c_id, 0), Binding::bind(a_id, 0));
        graph.set_input_binding(InputPort::new(c_id, 1), Binding::bind(b_id, 0));
        graph.set_input_binding(InputPort::new(print_id, 0), Binding::bind(c_id, 0));

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_sinks().await?;

        assert_eq!(*captured.lock().unwrap(), vec![6]); // 2 + 4
        Ok(())
    }

    /// An interior branch feeding an unconsumed composite output is pruned —
    /// its source func never runs (its hook would panic if it did).
    #[tokio::test(flavor = "multi_thread")]
    async fn dead_interior_branch_is_pruned() -> anyhow::Result<()> {
        let captured = Arc::new(StdMutex::new(Vec::<i64>::new()));
        let hooks = TestFuncHooks {
            get_a: Arc::new(|| Ok(7)),
            get_b: Arc::new(|| panic!("get_b feeds an unconsumed output and must be pruned")),
            print: {
                let c = captured.clone();
                Arc::new(move |v| c.lock().unwrap().push(v))
            },
        };
        let library = test_func_lib(hooks);

        // def TwoSources: get_a -> out0, get_b -> out1 (no inputs).
        let src_a = fnode(&library, "get_a");
        let src_b = fnode(&library, "get_b");
        let out = Node::new(NodeKind::GraphOutput);
        let mut def_graph = Graph::new("TwoSources")
            .category("Test")
            .outputs([int_out("O0"), int_out("O1")]);
        let sa = def_graph.add(src_a);
        let sb = def_graph.add(src_b);
        let out_id = def_graph.add(out);
        def_graph.set_input_binding(InputPort::new(out_id, 0), Binding::bind(sa, 0));
        def_graph.set_input_binding(InputPort::new(out_id, 1), Binding::bind(sb, 0));
        // parent: C, print <- C.out0 (out1 unused).
        let print = fnode(&library, "Print");

        let mut graph = Graph::default();
        let c = local_instance(&mut graph, def_graph);
        let c_id = graph.add(c);
        let print_id = graph.add(print);
        graph.set_input_binding(InputPort::new(print_id, 0), Binding::bind(c_id, 0));

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_sinks().await?;

        assert_eq!(*captured.lock().unwrap(), vec![7]); // get_a only; get_b pruned
        Ok(())
    }

    /// A data cycle that runs through a composite boundary is caught by the
    /// existing cycle detector once flattened.
    #[tokio::test(flavor = "multi_thread")]
    async fn cross_boundary_cycle_detected() {
        let library = test_func_lib(default_hooks());
        let def = wrap_sum_def(&library);

        // C.in0 <- C.out0 (self-cycle through the composite); print <- C.out0
        // so the cyclic node is reachable from a sink.
        let print = fnode(&library, "Print");

        let mut graph = Graph::default();
        let c = local_instance(&mut graph, def);
        let c_id = graph.add(c);
        let print_id = graph.add(print);
        graph.set_input_binding(InputPort::new(c_id, 0), Binding::bind(c_id, 0));
        graph.set_input_binding(InputPort::new(print_id, 0), Binding::bind(c_id, 0));

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        let result = eg.execute_sinks().await;

        assert!(
            matches!(result, Err(Error::CycleDetected { .. })),
            "expected CycleDetected, got {result:?}"
        );
    }

    fn bind_target(eg: &ExecutionEngine, e: &ExecutionNode, input_idx: usize) -> NodeId {
        match &eg.node_inputs(e)[input_idx].binding {
            ExecutionBinding::Bind(addr) => addr.target,
            other => panic!("expected Bind, got {other:?}"),
        }
    }

    /// A composite dissolves: only its interior func leaves remain, wired
    /// directly to the parent's producers/consumers.
    #[test]
    fn composite_dissolves_into_leaf_edges() {
        // get_a, get_b -> C(WrapSum) -> print.
        let library = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum_def(&library);

        let get_a = fnode(&library, "get_a");
        let get_b = fnode(&library, "get_b");
        let print = fnode(&library, "Print");

        let mut graph = Graph::default();
        let c = local_instance(&mut graph, def);
        let a_id = graph.add(get_a);
        let b_id = graph.add(get_b);
        let c_id = graph.add(c);
        let print_id = graph.add(print);
        graph.set_input_binding(InputPort::new(c_id, 0), Binding::bind(a_id, 0));
        graph.set_input_binding(InputPort::new(c_id, 1), Binding::bind(b_id, 0));
        graph.set_input_binding(InputPort::new(print_id, 0), Binding::bind(c_id, 0));

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // get_a, get_b, sum (interior), print — no composite/boundary nodes.
        assert_eq!(eg.compiled.program.e_nodes.len(), 4);
        let sum = eg.by_name("sum").unwrap();
        assert_eq!(bind_target(&eg, sum, 0), a_id);
        assert_eq!(bind_target(&eg, sum, 1), b_id);
        assert_eq!(bind_target(&eg, eg.by_name("Print").unwrap(), 0), sum.id);
    }

    /// A func-only graph builds with the node ids unchanged (caches survive).
    #[test]
    fn top_level_func_nodes_keep_identity() {
        let library = test_func_lib(default_hooks());
        let graph = test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        assert_eq!(eg.compiled.program.e_nodes.len(), graph.len());
        for node in graph.iter() {
            assert!(eg.by_id(&node.id).is_some(), "id preserved");
        }
    }

    /// Two instances of one def produce two distinct interior leaves.
    #[test]
    fn two_instances_get_distinct_leaf_ids() {
        let mut library = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum_def(&library);
        let def_id = GraphId::unique();
        library.insert_graph(def_id, def);

        let mut graph = Graph::default();
        let def_ref = library.graph_by_id(&def_id).unwrap();
        graph.add(Node::graph_instance(def_ref, GraphLink::Shared(def_id)));
        graph.add(Node::graph_instance(def_ref, GraphLink::Shared(def_id)));

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        let sums: Vec<NodeId> = eg
            .compiled
            .program
            .e_nodes
            .values()
            .filter(|e| e.name == "sum")
            .map(|e| e.id)
            .collect();
        assert_eq!(sums.len(), 2);
        assert_ne!(sums[0], sums[1]);
    }

    /// The `FlattenMap` maps a flattened interior node back to the
    /// editor's authoring ids: `attribution` yields the node's own id
    /// inside the def's graph, then each enclosing composite instance.
    /// This is what lets the editor show per-node stats inside a graph
    /// and accumulate them onto the instance node.
    #[test]
    fn flatten_map_attributes_interior_to_authoring_ids() {
        let library = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum_def(&library);
        // The id the editor knows the interior node by (in the def graph).
        let interior_sum_id = def.iter().find(|n| n.name == "sum").unwrap().id;

        let get_a = fnode(&library, "get_a");

        let mut graph = Graph::default();
        let c = local_instance(&mut graph, def);
        let a_id = graph.add(get_a);
        let c_id = graph.add(c);
        graph.set_input_binding(InputPort::new(c_id, 0), Binding::bind(a_id, 0));

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // Interior node: flattened id is remapped, but attribution points
        // back to the authoring interior id then the enclosing instance.
        let sum_flat = eg.by_name("sum").unwrap().id;
        assert_ne!(sum_flat, interior_sum_id, "flattened id is remapped");
        let attr: Vec<_> = eg.compiled.flatten_map.attribution(sum_flat).collect();
        assert_eq!(attr, vec![interior_sum_id, c_id]);

        // Top-level node: id unchanged, attribution is just itself.
        let a_attr: Vec<_> = eg.compiled.flatten_map.attribution(a_id).collect();
        assert_eq!(a_attr, vec![a_id]);
    }

    /// Add a `ticker` func (one event, no I/O) usable as an interior or parent
    /// emitter; instantiate it by name with `fnode`.
    fn add_ticker(library: &mut Library) {
        library.add(
            Func::new(FuncId::unique(), "ticker")
                .category("Test")
                .sink()
                .event("tick", EventLambda::default()),
        );
    }

    fn func_lib_with_ticker() -> Library {
        let mut library = test_func_lib(default_hooks());
        add_ticker(&mut library);
        library
    }

    fn subscriber_ids(eg: &ExecutionEngine, e: &ExecutionNode, event_idx: usize) -> Vec<NodeId> {
        eg.node_events(e)[event_idx].subscribers.clone()
    }

    /// A parent subscriber of a composite's exposed event is rewired onto the
    /// flattened interior emitter.
    #[test]
    fn exposed_event_rewires_parent_subscriber_to_interior_emitter() {
        let library = func_lib_with_ticker();

        // def: a single `ticker`, its `tick` event exposed as the composite's
        // event 0.
        let emitter = fnode(&library, "ticker");
        let mut def_graph = Graph::new("Exposer").category("Test");
        let emitter_id = def_graph.add(emitter);
        def_graph.events.push(GraphEvent {
            name: "tick".into(),
            emitter: emitter_id,
            emitter_event_idx: 0,
        });

        // parent: composite C, and `listener` subscribing to C's event 0.
        let listener = fnode(&library, "Print");

        let mut graph = Graph::default();
        let c = local_instance(&mut graph, def_graph);
        let c_id = graph.add(c);
        let listener_id = graph.add(listener);
        graph.subscribe(c_id, 0, listener_id);

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // The flattened interior `ticker` carries the rewired subscriber.
        let ticker_node = eg.by_name("ticker").unwrap();
        assert_eq!(subscriber_ids(&eg, ticker_node, 0), vec![listener_id]);
    }

    /// Triggering a composite (as a subscriber) reaches the interior nodes
    /// wired to its `GraphInput` trigger.
    #[test]
    fn triggering_composite_reaches_interior_subscribers() {
        let library = func_lib_with_ticker();

        // def: GraphInput trigger → interior `print` subscribes to it.
        let si = Node::new(NodeKind::GraphInput);
        let reactor = fnode(&library, "Print");
        let mut def_graph = Graph::new("Reactor").category("Test");
        let si_id = def_graph.add(si);
        let reactor_id = def_graph.add(reactor);
        def_graph.subscribe(si_id, 0, reactor_id);
        // parent: `ticker` emits; composite C subscribes to it.
        let emitter = fnode(&library, "ticker");

        let mut graph = Graph::default();
        let c = local_instance(&mut graph, def_graph);
        let emitter_id = graph.add(emitter);
        let c_id = graph.add(c);
        graph.subscribe(emitter_id, 0, c_id);

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // The interior `print` flat id is the one wired onto `ticker`'s event.
        let reactor_flat = eg.by_name("Print").unwrap().id;
        let ticker_node = eg.by_id(&emitter_id).unwrap();
        assert_eq!(subscriber_ids(&eg, ticker_node, 0), vec![reactor_flat]);
    }

    /// Editing a shared (linked) def re-inlines every instance on the next
    /// update, and the interior leaves keep their flat ids (so caches persist).
    #[tokio::test(flavor = "multi_thread")]
    async fn editing_linked_def_propagates_to_all_instances() -> anyhow::Result<()> {
        let captured = Arc::new(StdMutex::new(Vec::<i64>::new()));
        let hooks = TestFuncHooks {
            get_a: Arc::new(|| Ok(0)),
            get_b: Arc::new(|| 0),
            print: {
                let c = captured.clone();
                Arc::new(move |v| c.lock().unwrap().push(v))
            },
        };
        let mut library = test_func_lib(hooks);
        let def = wrap_sum_def(&library);
        let def_id = GraphId::unique();
        library.insert_graph(def_id, def);

        // Two linked instances with const inputs, each feeding a print.
        let def_ref = library.graph_by_id(&def_id).unwrap();
        let c1 = Node::graph_instance(def_ref, GraphLink::Shared(def_id));
        let c2 = Node::graph_instance(def_ref, GraphLink::Shared(def_id));
        let p1 = fnode(&library, "Print");
        let p2 = fnode(&library, "Print");

        let mut graph = Graph::default();
        let c1_id = graph.add(c1);
        let c2_id = graph.add(c2);
        let p1_id = graph.add(p1);
        let p2_id = graph.add(p2);
        graph.set_input_binding(InputPort::new(c1_id, 0), StaticValue::Int(1).into());
        graph.set_input_binding(InputPort::new(c1_id, 1), StaticValue::Int(2).into());
        graph.set_input_binding(InputPort::new(c2_id, 0), StaticValue::Int(10).into());
        graph.set_input_binding(InputPort::new(c2_id, 1), StaticValue::Int(20).into());
        graph.set_input_binding(InputPort::new(p1_id, 0), Binding::bind(c1_id, 0));
        graph.set_input_binding(InputPort::new(p2_id, 0), Binding::bind(c2_id, 0));

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_sinks().await?;

        let mut got = captured.lock().unwrap().clone();
        got.sort();
        assert_eq!(got, vec![3, 30]); // sums: 1+2, 10+20
        captured.lock().unwrap().clear();

        // Interior `sum` flat ids (sorted) — for the cache-stability check.
        let sum_ids = |eg: &ExecutionEngine| -> Vec<NodeId> {
            let mut ids: Vec<NodeId> = eg
                .compiled
                .program
                .e_nodes
                .values()
                .filter(|e| e.name == "sum")
                .map(|e| e.id)
                .collect();
            ids.sort();
            ids
        };
        let sum_ids_before = sum_ids(&eg);
        assert_eq!(sum_ids_before.len(), 2);

        // Edit the linked def: route the exposed output straight from input A
        // (passthrough) instead of `sum`. Affects both instances.
        {
            let graph = library.graphs.get_mut(&def_id).unwrap();
            let si = graph
                .iter()
                .find(|n| matches!(n.kind, NodeKind::GraphInput))
                .unwrap()
                .id;
            let so = graph
                .iter()
                .find(|n| matches!(n.kind, NodeKind::GraphOutput))
                .unwrap()
                .id;
            graph.set_input_binding(InputPort::new(so, 0), Binding::bind(si, 0));
        }

        eg.update(&graph, &library).unwrap();
        eg.execute_sinks().await?;

        let mut got = captured.lock().unwrap().clone();
        got.sort();
        assert_eq!(got, vec![1, 10]); // now passthrough of input A

        // Same interior `sum` leaves, same ids → caches were preserved, not
        // rebuilt under fresh keys.
        assert_eq!(sum_ids_before, sum_ids(&eg));
        Ok(())
    }

    /// An event fired at a parent emitter reaches, through the real execution
    /// path, the interior nodes wired to a subscribed composite's trigger.
    #[tokio::test(flavor = "multi_thread")]
    async fn event_through_composite_triggers_interior_node() -> anyhow::Result<()> {
        let ran = Arc::new(StdMutex::new(0i64));
        let hooks = TestFuncHooks {
            get_a: {
                let r = ran.clone();
                Arc::new(move || {
                    *r.lock().unwrap() += 1;
                    Ok(1)
                })
            },
            get_b: Arc::new(|| 0),
            print: Arc::new(|_| {}),
        };
        let mut library = test_func_lib(hooks);
        add_ticker(&mut library);

        // def Reactor: GraphInput trigger → interior `get_a` subscribes.
        let si = Node::new(NodeKind::GraphInput);
        let reactor = fnode(&library, "get_a");
        let mut def_graph = Graph::new("Reactor").category("Test");
        let si_id = def_graph.add(si);
        let reactor_id = def_graph.add(reactor);
        def_graph.subscribe(si_id, 0, reactor_id);
        // parent: `ticker` E; composite C subscribes to E's event.
        let emitter = fnode(&library, "ticker");

        let mut graph = Graph::default();
        let c = local_instance(&mut graph, def_graph);
        let emitter_id = graph.add(emitter);
        let c_id = graph.add(c);
        graph.subscribe(emitter_id, 0, c_id);

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // Fire E's event (as the worker does) — the interior `get_a` runs.
        eg.execute_events([EventRef {
            node_id: emitter_id,
            event_idx: 0,
        }])
        .await?;

        assert_eq!(*ran.lock().unwrap(), 1);
        Ok(())
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
    use crate::library::{Library, TypeEntry};
    use crate::node::definition::{Func, FuncInput, FuncOutput};
    use crate::{CustomValue, TypeId};

    const TRACKED_TYPE: &str = "7266406a-8083-4e46-b661-de4308bcec96";
    const RELAY_FUNC: &str = "2b16e013-11fb-49cf-89b1-a9cb54c06be3";
    const SINK_FUNC: &str = "ec454492-e235-4b49-b3ef-ae0b2b85bf5f";

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

    /// A `relay` (pure, custom→custom, emits a fresh `Tracked`) + a `sink` that
    /// consumes one, over the shared `tracker`.
    fn relay_library(tracker: Arc<StdMutex<LiveTracker>>) -> Library {
        let mut library = Library::default();
        library.register_type(TRACKED_TYPE, TypeEntry::custom("Tracked"));
        let tracker_l = tracker.clone();
        library.add(
            Func::new(RELAY_FUNC, "relay")
                .category("Test")
                .pure()
                .input(FuncInput::optional(
                    "in",
                    DataType::Custom(TRACKED_TYPE.into()),
                ))
                .output(FuncOutput::new(
                    "out",
                    DataType::Custom(TRACKED_TYPE.into()),
                ))
                .lambda(async_lambda!(
                    move |_, _, _, _, _, outputs| { tracker = tracker_l.clone() } => {
                        outputs[0] = DynamicValue::Custom(Arc::new(Tracked::new(tracker.clone())));
                        Ok(())
                    }
                )),
        );
        library.add(
            Func::new(SINK_FUNC, "sink")
                .category("Test")
                .sink()
                .input(FuncInput::required(
                    "in",
                    DataType::Custom(TRACKED_TYPE.into()),
                ))
                .lambda(async_lambda!(|_, _, _, _, _, _| { Ok(()) })),
        );
        library
    }

    /// Run a 4-stage relay chain into a sink with every relay set to `relay_mode`, and return
    /// the peak number of tracked outputs resident at once.
    async fn chain_peak(relay_mode: CacheMode) -> usize {
        let tracker = Arc::new(StdMutex::new(LiveTracker::default()));
        let library = relay_library(tracker.clone());

        let relays: Vec<NodeId> = (0..4).map(|_| NodeId::unique()).collect();
        let sink_id = NodeId::unique();
        let mut graph = Graph::default();
        for &id in &relays {
            let mut n = node(&library, "relay");
            n.cache = relay_mode;
            graph.insert(id, n);
        }
        graph.insert(sink_id, node(&library, "sink"));
        for pair in relays.windows(2) {
            graph.set_input_binding(InputPort::new(pair[1], 0), Binding::bind(pair[0], 0));
        }
        graph.set_input_binding(InputPort::new(sink_id, 0), Binding::bind(relays[3], 0));
        graph.debug_check();

        let mut engine = ExecutionEngine::default();
        engine.update(&graph, &library).unwrap();
        engine.execute_sinks().await.unwrap();

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

    const PROBE_FUNC: &str = "a19f251a-465c-4a05-b9e3-f4a4c2389733";

    /// [`relay_library`] plus a `probe` sink that takes its input value out of the
    /// invoke buffer and records whether it was uniquely owned (`into_custom` succeeded)
    /// — the observable contract of the executor's move-on-last-use.
    fn probe_library(
        tracker: Arc<StdMutex<LiveTracker>>,
        unique_reads: Arc<StdMutex<Vec<bool>>>,
    ) -> Library {
        let mut library = relay_library(tracker);
        library.add(
            Func::new(PROBE_FUNC, "probe")
                .category("Test")
                .sink()
                .input(FuncInput::required(
                    "in",
                    DataType::Custom(TRACKED_TYPE.into()),
                ))
                .lambda(async_lambda!(
                    move |_, _, _, inputs, _, _| { reads = unique_reads.clone() } => {
                        let value = std::mem::take(&mut inputs[0].value);
                        reads
                            .lock()
                            .unwrap()
                            .push(value.into_custom::<Tracked>().is_ok());
                        Ok(())
                    }
                )),
        );
        library
    }

    /// Each probe's ownership observation, in invocation order, plus what stayed live.
    struct ProbeRun {
        unique_reads: Vec<bool>,
        live_after: usize,
    }

    /// Run `relay → n_probes × probe` with the relay in `relay_mode`.
    async fn probe_run(relay_mode: CacheMode, n_probes: usize) -> ProbeRun {
        let tracker = Arc::new(StdMutex::new(LiveTracker::default()));
        let unique_reads = Arc::new(StdMutex::new(Vec::new()));
        let library = probe_library(tracker.clone(), unique_reads.clone());

        let relay_id = NodeId::unique();
        let mut graph = Graph::default();
        let mut relay = node(&library, "relay");
        relay.cache = relay_mode;
        graph.insert(relay_id, relay);
        for _ in 0..n_probes {
            let probe_id = NodeId::unique();
            graph.insert(probe_id, node(&library, "probe"));
            graph.set_input_binding(InputPort::new(probe_id, 0), Binding::bind(relay_id, 0));
        }
        graph.debug_check();

        let mut engine = ExecutionEngine::default();
        engine.update(&graph, &library).unwrap();
        engine.execute_sinks().await.unwrap();

        ProbeRun {
            unique_reads: unique_reads.lock().unwrap().clone(),
            live_after: tracker.lock().unwrap().current,
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
    use crate::async_lambda;
    use crate::graph::Graph;
    use crate::graph::NodeKind;
    use crate::graph::interface::{GraphId, GraphLink};
    use crate::node::definition::{Func, FuncInput, FuncOutput};
    use std::sync::Mutex as StdMutex;

    /// The output-type pool is span-addressed: when a consumer precedes its producer
    /// in insertion order, flatten's `set_input` claims the producer's *index* early
    /// while output spans are assigned in emit order — an index-order sequential fill
    /// would hand the two producers each other's types.
    #[test]
    fn output_types_follow_spans_when_consumer_precedes_producer() {
        let library: Library = [
            Func::new("7ab6d0c9-8c35-4364-b2e3-62ab1ba5a888", "make_int")
                .category("Test")
                .pure()
                .output(FuncOutput::new("V", DataType::Int))
                .lambda(async_lambda!(|_, _, _, _, _, outputs| {
                    outputs[0] = StaticValue::Int(1).into();
                    Ok(())
                })),
            Func::new("cbac49ae-bbb0-48d7-a586-086815a487a6", "make_str")
                .category("Test")
                .pure()
                .output(FuncOutput::new("V", DataType::String))
                .lambda(async_lambda!(|_, _, _, _, _, outputs| {
                    outputs[0] = StaticValue::String("s".into()).into();
                    Ok(())
                })),
            Func::new("001fccec-5732-41c6-b448-379d4cf40dc3", "sink")
                .category("Test")
                .sink()
                .input(FuncInput::required("In", DataType::Any))
                .lambda(async_lambda!(|_, _, _, _, _, _| { Ok(()) })),
        ]
        .into();

        // Insertion order: the consumer first, then the *other* producer, then the
        // producer it binds — so `make_str` claims flat index 1 while its output span
        // is assigned last.
        let mut graph = Graph::default();
        graph.add(node(&library, "sink"));
        graph.add(node(&library, "make_int"));
        graph.add(node(&library, "make_str"));
        let sink_id = graph.find_by_name("sink", NodeSearch::TopLevel).unwrap().id;
        let str_id = graph
            .find_by_name("make_str", NodeSearch::TopLevel)
            .unwrap()
            .id;
        graph.set_input_binding(InputPort::new(sink_id, 0), Binding::bind(str_id, 0));

        let mut engine = ExecutionEngine::default();
        engine.update(&graph, &library).unwrap();

        let make_int = engine.by_name("make_int").unwrap();
        let make_str = engine.by_name("make_str").unwrap();
        assert_eq!(
            engine.compiled.program.node_output_types(make_int),
            &[DataType::Int],
            "make_int reads its own type, not its neighbor's"
        );
        assert_eq!(
            engine.compiled.program.node_output_types(make_str),
            &[DataType::String],
            "make_str reads its own type, not its neighbor's"
        );
    }

    /// An `Update` may carry an evolved library: changed inputs and lambdas must
    /// replace their prior compiled forms under the reused flat node.
    #[tokio::test]
    async fn update_with_evolved_func_recompiles_and_runs_new_lambda() {
        let printed = Arc::new(StdMutex::new(Vec::<i64>::new()));
        let make_lib = |result: i64, extra_input: bool| {
            let mut lib = test_func_lib(TestFuncHooks {
                print: {
                    let p = printed.clone();
                    Arc::new(move |v| p.lock().unwrap().push(v))
                },
                ..default_hooks()
            });
            let mut generator = Func::new("3cb06374-2a86-45e1-91db-fec227538a97", "generate")
                .category("Test")
                .pure()
                .output(FuncOutput::new("V", DataType::Int))
                .lambda(async_lambda!(move |_, _, _, _, _, outputs| {
                    outputs[0] = StaticValue::Int(result).into();
                    Ok(())
                }));
            if extra_input {
                generator = generator.input(FuncInput::optional("Extra", DataType::Int));
            }
            lib.add(generator);
            lib
        };

        let lib_v1 = make_lib(1, false);
        let mut graph = Graph::default();
        graph.add(node(&lib_v1, "generate"));
        graph.add(node(&lib_v1, "Print"));
        let generate_id = graph
            .find_by_name("generate", NodeSearch::TopLevel)
            .unwrap()
            .id;
        let print_id = graph
            .find_by_name("Print", NodeSearch::TopLevel)
            .unwrap()
            .id;
        graph.set_input_binding(InputPort::new(print_id, 0), Binding::bind(generate_id, 0));

        let mut engine = ExecutionEngine::default();
        engine.update(&graph, &lib_v1).unwrap();
        engine.execute_sinks().await.unwrap();
        assert_eq!(*printed.lock().unwrap(), vec![1], "v1 lambda ran");

        // v2: same FuncId, one more input, different lambda.
        let lib_v2 = make_lib(2, true);
        engine.update(&graph, &lib_v2).unwrap();
        assert_eq!(
            engine
                .node_inputs(engine.by_name("generate").unwrap())
                .len(),
            1,
            "the reused flat node picked up the grown input list"
        );
        engine.execute_sinks().await.unwrap();
        assert_eq!(
            *printed.lock().unwrap(),
            vec![1, 2],
            "the input-shape change re-keyed the digest and the new lambda ran"
        );
    }

    /// Inspecting a node *inside* a graph goes by its authoring id — the flat id
    /// is hashed from the descent path, so the query must resolve through the
    /// flatten map instead of missing and silently returning nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn interior_node_inspection_resolves_authoring_id() {
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 4),
            print: Arc::new(|_| {}),
        });

        // def: in(A,B) -> sum -> out
        let in_node = Node::new(NodeKind::GraphInput);
        let sum: Node = library.by_name("sum").unwrap().into();
        let out = Node::new(NodeKind::GraphOutput);
        let mut def_graph = Graph::new("WrapSum")
            .category("Test")
            .input(FuncInput::required("A", DataType::Int))
            .input(FuncInput::optional("B", DataType::Int))
            .output(FuncOutput::new("Sum", DataType::Int));
        let in_id = def_graph.add(in_node);
        let sum_interior_id = def_graph.add(sum);
        let out_id = def_graph.add(out);
        def_graph.set_input_binding(InputPort::new(sum_interior_id, 0), Binding::bind(in_id, 0));
        def_graph.set_input_binding(InputPort::new(sum_interior_id, 1), Binding::bind(in_id, 1));
        def_graph.set_input_binding(InputPort::new(out_id, 0), Binding::bind(sum_interior_id, 0));
        let get_a: Node = library.by_name("get_a").unwrap().into();
        let get_b: Node = library.by_name("get_b").unwrap().into();
        let graph_id = GraphId::unique();
        let inst = Node::graph_instance(&def_graph, GraphLink::Local(graph_id));
        let print: Node = library.by_name("Print").unwrap().into();

        let mut graph = Graph::default();
        graph.insert_graph(graph_id, def_graph);
        let a_id = graph.add(get_a);
        let b_id = graph.add(get_b);
        let inst_id = graph.add(inst);
        let print_id = graph.add(print);
        graph.set_input_binding(InputPort::new(inst_id, 0), Binding::bind(a_id, 0));
        graph.set_input_binding(InputPort::new(inst_id, 1), Binding::bind(b_id, 0));
        graph.set_input_binding(InputPort::new(print_id, 0), Binding::bind(inst_id, 0));

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_sinks().await.unwrap();

        assert!(
            eg.by_id(&sum_interior_id).is_none(),
            "interior ids are remapped at flatten — the key lookup alone must miss"
        );
        let values = eg
            .get_argument_values(&sum_interior_id)
            .expect("the interior node resolves through the flatten map");
        assert_eq!(
            values.outputs[0].as_i64(),
            Some(6),
            "2 + 4 computed inside the composite"
        );
    }
}
