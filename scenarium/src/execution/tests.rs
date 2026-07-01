use std::sync::Arc;

use super::*;
use crate::data::{DataType, DynamicValue, StaticValue};
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::function::FuncBehavior;
use crate::graph::{Binding, CachePersistence, InputPort, Node};
use crate::testing::{TestFuncHooks, test_func_lib, test_graph};
use common::{FloatExt, SerdeFormat};
use tokio::sync::Mutex;

// === Shared Helpers ===

fn execution_node_names_in_order(execution_graph: &ExecutionEngine) -> Vec<String> {
    execution_graph
        .plan
        .execute_order
        .iter()
        .map(|&e_node_idx| execution_graph.program.e_nodes[e_node_idx].name.clone())
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
fn node(library: &Library, func_name: &str, id: NodeId) -> Node {
    let mut node: Node = library.by_name(func_name).unwrap().into();
    node.id = id;
    node
}

/// Set input `idx` of the named node's binding in the source graph.
fn bind(graph: &mut Graph, node_name: &str, idx: usize, binding: Binding) {
    let id = graph.by_name(node_name).unwrap().id;
    graph.set_input_binding(InputPort::new(id, idx), binding);
}

// === Disk Cache (engine integration) ===

mod cache_persistence {
    use super::*;
    use crate::execution::cache::ValueCache;
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

    /// A fresh engine backed by a content-addressed store rooted at `dir`
    /// (simulating a reopen when called twice against the same dir). The default
    /// empty library is fine — these tests cache plain values.
    fn disk_engine(dir: &TempDir) -> ExecutionEngine {
        use crate::execution::output_cache::OutputCache;
        use crate::library::Library;
        use std::sync::Arc;
        let mut engine = ExecutionEngine::default();
        engine.set_output_cache(OutputCache::new(
            Arc::new(Library::default()),
            Some(dir.0.clone()),
        ));
        engine
    }

    /// A `persist` node's output survives a fresh engine (reopen), its sole-consumer
    /// upstream is pruned on the hit, and a digest change (a bumped func version,
    /// standing in for any input change) invalidates it.
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

        // get_a (pure source) → mult (pure, persist Disk) → print (terminal).
        let lib = make_lib();
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a", NodeId::unique()));
        let mut mult = node(&lib, "mult", NodeId::unique());
        mult.persist = CachePersistence::Disk;
        graph.add(mult);
        graph.add(node(&lib, "print", NodeId::unique()));
        let get_a_id = graph.by_name("get_a").unwrap().id;
        let mult_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, (get_a_id, 0).into());
        bind(&mut graph, "mult", 1, (get_a_id, 0).into());
        bind(&mut graph, "print", 0, (mult_id, 0).into());

        // First run: everything computes; `mult` is stored to disk.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        engine.execute_terminals().await.unwrap();
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 1);

        // Reopen: a fresh engine over the same store. `mult` loads from disk
        // (cached) and `get_a` is pruned — never re-runs.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        let stats = engine.execute_terminals().await.unwrap();
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            1,
            "get_a must not recompute"
        );
        assert!(
            stats.cached_nodes.contains(&mult_id),
            "mult should be disk-cached"
        );
        assert!(
            !stats.executed_nodes.iter().any(|n| n.node_id == get_a_id),
            "mult's sole-consumer upstream should be pruned"
        );

        // Digest change: bump `mult`'s func version ⇒ miss ⇒ recompute ⇒ get_a runs.
        let mut bumped = make_lib();
        bumped.by_name_mut("mult").unwrap().version = 1;
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &bumped).unwrap();
        let stats = engine.execute_terminals().await.unwrap();
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            2,
            "version bump must recompute"
        );
        assert!(
            !stats.cached_nodes.contains(&mult_id),
            "mult should not be cached after a digest change"
        );
    }

    /// Two disk-cached nodes chained (`sum` → `mult`) under an executing terminal
    /// (`print`). On reopen only the frontier `mult` — the cached value `print`
    /// actually reads — is deserialized into RAM; the deeper `sum`, whose sole
    /// consumer `mult` is itself cached, stays on disk (`disk_available`, no resident
    /// bytes). That is the RAM win. Inspecting `sum` then pulls it in on demand.
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

        // get_a(7) → sum(persist) = 7+7 = 14 → mult(persist) = 14*7 = 98 → print.
        let lib = make_lib();
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a", NodeId::unique()));
        let mut sum = node(&lib, "sum", NodeId::unique());
        sum.persist = CachePersistence::Disk;
        graph.add(sum);
        let mut mult = node(&lib, "mult", NodeId::unique());
        mult.persist = CachePersistence::Disk;
        graph.add(mult);
        graph.add(node(&lib, "print", NodeId::unique()));
        let get_a_id = graph.by_name("get_a").unwrap().id;
        let sum_id = graph.by_name("sum").unwrap().id;
        let mult_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "sum", 0, (get_a_id, 0).into());
        bind(&mut graph, "sum", 1, (get_a_id, 0).into());
        bind(&mut graph, "mult", 0, (sum_id, 0).into());
        bind(&mut graph, "mult", 1, (get_a_id, 0).into());
        bind(&mut graph, "print", 0, (mult_id, 0).into());

        // First run: everything computes; sum (14) and mult (98) stored to disk.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        engine.execute_terminals().await.unwrap();
        assert_eq!(get_a_calls.load(Ordering::SeqCst), 1);

        // Reopen over the same store with fresh RAM, then run.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        let stats = engine.execute_terminals().await.unwrap();

        // Both disk-cached nodes are served (pruned); get_a never recomputes.
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            1,
            "no upstream recompute"
        );
        assert!(
            stats.cached_nodes.contains(&sum_id) && stats.cached_nodes.contains(&mult_id),
            "both disk-cached nodes are pruned"
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
            ValueCache::OnDisk
        );
        assert!(
            !sum_resident,
            "a disk cache behind another is not loaded into RAM"
        );
        assert!(
            sum_on_disk,
            "the deeper cache is still flagged available on disk"
        );

        // Inspecting `sum` pulls it in on demand: value correct, now resident.
        let vals = engine
            .get_argument_values_with_previews(&sum_id)
            .await
            .unwrap();
        assert!(
            matches!(vals.outputs[0], DynamicValue::Static(StaticValue::Int(14))),
            "inspection reads sum's stored value: {:?}",
            vals.outputs
        );
        assert!(
            engine
                .runtime_slot(engine.by_name("sum").unwrap())
                .output_values()
                .is_some(),
            "inspection hydrated sum into RAM"
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

        // get_a(1) → sum(persist) = 2 → mult(persist) = 2 → print, one engine.
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a", NodeId::unique()));
        let mut sum = node(&lib, "sum", NodeId::unique());
        sum.persist = CachePersistence::Disk;
        graph.add(sum);
        let mut mult = node(&lib, "mult", NodeId::unique());
        mult.persist = CachePersistence::Disk;
        graph.add(mult);
        graph.add(node(&lib, "print", NodeId::unique()));
        let get_a_id = graph.by_name("get_a").unwrap().id;
        let sum_id = graph.by_name("sum").unwrap().id;
        let mult_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "sum", 0, (get_a_id, 0).into());
        bind(&mut graph, "sum", 1, (get_a_id, 0).into());
        bind(&mut graph, "mult", 0, (sum_id, 0).into());
        bind(&mut graph, "mult", 1, (get_a_id, 0).into());
        bind(&mut graph, "print", 0, (mult_id, 0).into());

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &lib).unwrap();

        // Run 1 (cold): everything computes and stays resident; nothing evicted yet
        // (all of it is this run's own output).
        engine.execute_terminals().await.unwrap();
        assert!(
            engine
                .runtime_slot(engine.by_name("sum").unwrap())
                .output_values()
                .is_some(),
            "sum is resident after the run that computed it"
        );

        // Run 2: only `print` runs, reading cached `mult` (frontier). `sum` is now
        // an untouched, reloadable leftover.
        let stats = engine.execute_terminals().await.unwrap();
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
            ValueCache::OnDisk
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

        // Lossless: inspecting `sum` reloads it with the correct value (1 + 1 = 2).
        let vals = engine
            .get_argument_values_with_previews(&sum_id)
            .await
            .unwrap();
        assert!(
            matches!(vals.outputs[0], DynamicValue::Static(StaticValue::Int(2))),
            "evicted value reloads from disk on inspection: {:?}",
            vals.outputs
        );
    }

    /// Flipping a `persist` node's inputs *back* to a previously-stored configuration
    /// must serve that config's value from its disk blob — not the stale RAM value the
    /// intervening run left resident under a now-superseded digest. The old
    /// `(output_values, output_digest, disk_available)` slot let that stale RAM value
    /// mask the fresh blob at hydrate (`hydrate_slot` short-circuited on "values
    /// present"); the `ValueCache` enum drops it when `mark_available` flags the blob,
    /// so the pruned frontier loads the correct bytes.
    #[tokio::test(flavor = "multi_thread")]
    async fn flip_back_to_stored_digest_serves_disk_blob_not_stale_ram() -> anyhow::Result<()> {
        let dir = TempDir::new("flip_back");
        let lib = test_func_lib(default_hooks());

        // mult(persist=Disk) read by print. Const binds detach mult from any upstream,
        // so its digest is a pure function of the two consts. Fixed node ids so the
        // slot (and its resident value) survives each `update`.
        let mut graph = Graph::default();
        let mut mult = node(&lib, "mult", NodeId::from_u128(1));
        mult.persist = CachePersistence::Disk;
        graph.add(mult);
        graph.add(node(&lib, "print", NodeId::from_u128(2)));
        let mult_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "print", 0, (mult_id, 0).into());

        let set = |graph: &mut Graph, a: i64, b: i64| {
            bind(graph, "mult", 0, Binding::Const(StaticValue::Int(a)));
            bind(graph, "mult", 1, Binding::Const(StaticValue::Int(b)));
        };

        let mut engine = disk_engine(&dir);

        // Config A: mult = 2 * 3 = 6 → blob_A stored on disk.
        set(&mut graph, 2, 3);
        engine.update(&graph, &lib)?;
        engine.execute_terminals().await?;

        // Config B: mult = 5 * 7 = 35 → slot now resident with 35 under B's digest,
        // and blob_B stored at a different (content-addressed) path.
        set(&mut graph, 5, 7);
        engine.update(&graph, &lib)?;
        engine.execute_terminals().await?;

        // Flip back to A: blob_A is still on disk, but the slot holds 35 in RAM under
        // B's (now superseded) digest. mult is pruned (disk hit) and read as print's
        // frontier — it must serve 6 from disk, not the stale 35.
        set(&mut graph, 2, 3);
        engine.update(&graph, &lib)?;
        let stats = engine.execute_terminals().await?;

        // mult is served from its disk blob, not recomputed — without this, a recompute
        // would yield 6 regardless and the stale-RAM path would go untested.
        assert!(
            !stats.executed_nodes.iter().any(|n| n.node_id == mult_id),
            "mult is a disk cache hit on flip-back, not recomputed: {:?}",
            stats.executed_nodes
        );

        let vals = engine
            .get_argument_values_with_previews(&mult_id)
            .await
            .unwrap();
        assert!(
            matches!(vals.outputs[0], DynamicValue::Static(StaticValue::Int(6))),
            "flip-back serves the disk blob (6), not the stale RAM value (35): {:?}",
            vals.outputs
        );
        Ok(())
    }

    /// A `persist` node is written to disk the moment *it* finishes, not in a batch at
    /// the end of the run — so its blob is already on disk by the time a downstream
    /// node executes. The terminal `print` hook checks the store dir is non-empty when
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
        let mut mult = node(&lib, "mult", NodeId::unique());
        mult.persist = CachePersistence::Disk;
        graph.add(mult);
        graph.add(node(&lib, "print", NodeId::unique()));
        let mult_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(2)));
        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(3)));
        bind(&mut graph, "print", 0, (mult_id, 0).into());

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &lib).unwrap();
        engine.execute_terminals().await.unwrap();

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
        let build = |persist: CachePersistence| {
            let mut graph = Graph::default();
            let mut mult = node(&lib, "mult", NodeId::from_u128(1));
            mult.persist = persist;
            graph.add(mult);
            graph.add(node(&lib, "print", NodeId::from_u128(2)));
            let mult_id = graph.by_name("mult").unwrap().id;
            bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(2)));
            bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(3)));
            bind(&mut graph, "print", 0, (mult_id, 0).into());
            graph
        };

        let mut engine = disk_engine(&dir);

        // Run with Memory caching: mult computes (6) and stays resident, nothing on disk.
        engine
            .update(&build(CachePersistence::Memory), &lib)
            .unwrap();
        engine.execute_terminals().await.unwrap();
        assert!(
            std::fs::read_dir(&dir.0).unwrap().next().is_none(),
            "Memory caching writes nothing to disk"
        );

        // Toggle the node to Disk (a graph edit → update), but do NOT re-run. The
        // resident value must reach disk now, not on some later execution.
        engine.update(&build(CachePersistence::Disk), &lib).unwrap();
        engine.store_resident_caches().await;
        assert!(
            std::fs::read_dir(&dir.0).unwrap().next().is_some(),
            "toggling Disk persists the resident value without a re-execution"
        );
    }

    /// `store_resident_caches` must not write a value under a digest it wasn't produced
    /// under. After an input change recompiles the program, a node's resident value is
    /// stale w.r.t. its new digest; flushing it to the new digest's content-addressed
    /// path would make a later run at that digest load stale bytes.
    #[tokio::test]
    async fn flush_skips_a_value_stale_for_the_current_digest() {
        let dir = TempDir::new("stale_flush");
        let lib = test_func_lib(default_hooks());

        // mult(persist=Disk) with const inputs → print; the consts drive mult's digest.
        let build = |a: i64, b: i64| {
            let mut graph = Graph::default();
            let mut mult = node(&lib, "mult", NodeId::from_u128(1));
            mult.persist = CachePersistence::Disk;
            graph.add(mult);
            graph.add(node(&lib, "print", NodeId::from_u128(2)));
            let mult_id = graph.by_name("mult").unwrap().id;
            bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(a)));
            bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(b)));
            bind(&mut graph, "print", 0, (mult_id, 0).into());
            graph
        };
        let blob_count = |dir: &TempDir| std::fs::read_dir(&dir.0).unwrap().count();

        let mut engine = disk_engine(&dir);

        // Config A: mult runs and is stored under its digest D_A (one blob).
        engine.update(&build(2, 3), &lib).unwrap();
        engine.execute_terminals().await.unwrap();
        assert_eq!(blob_count(&dir), 1, "config A's blob is stored");

        // Config B: mult's inputs change ⇒ its *current* digest is now D_B, but the
        // resident value (6) was produced under D_A. Recompile (update), no re-run, then
        // flush — the stale value must not be written under D_B.
        engine.update(&build(5, 7), &lib).unwrap();
        engine.store_resident_caches().await;
        assert_eq!(
            blob_count(&dir),
            1,
            "a value stale for the current digest is not flushed (no second, D_B blob)"
        );
    }

    /// A corrupt / incompatible cache blob must be *deleted* on a failed load, so the
    /// recompute that follows writes a fresh one. Without the delete, `store_node`'s
    /// skip-if-exists keeps the broken file and the node recomputes on *every* run
    /// (the regression: an old-format blob rejected by `FORMAT_VERSION` was never
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
        graph.add(node(&lib, "get_a", NodeId::from_u128(1)));
        let mut mult = node(&lib, "mult", NodeId::from_u128(2));
        mult.persist = CachePersistence::Disk;
        graph.add(mult);
        graph.add(node(&lib, "print", NodeId::from_u128(3)));
        let get_a_id = graph.by_name("get_a").unwrap().id;
        let mult_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, (get_a_id, 0).into());
        bind(&mut graph, "mult", 1, (get_a_id, 0).into());
        bind(&mut graph, "print", 0, (mult_id, 0).into());

        // Cold run: get_a + mult compute; mult is stored (one blob).
        {
            let mut engine = disk_engine(&dir);
            engine.update(&graph, &lib).unwrap();
            engine.execute_terminals().await.unwrap();
        }
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            1,
            "cold run computes get_a"
        );

        // Corrupt mult's blob (a torn write / an old, version-mismatched format).
        let blob = std::fs::read_dir(&dir.0)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(&blob, b"garbage").unwrap();

        // Reopen: mult's blob fails to load → recompute (get_a runs again) → the corrupt
        // blob is deleted and a fresh one written.
        {
            let mut engine = disk_engine(&dir);
            engine.update(&graph, &lib).unwrap();
            engine.execute_terminals().await.unwrap();
        }
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            2,
            "a corrupt blob forces exactly one recompute"
        );

        // Reopen again: mult's blob is fresh → cache hit → get_a stays pruned.
        {
            let mut engine = disk_engine(&dir);
            engine.update(&graph, &lib).unwrap();
            engine.execute_terminals().await.unwrap();
        }
        assert_eq!(
            get_a_calls.load(Ordering::SeqCst),
            2,
            "the corrupt blob was replaced, so the next reopen is a hit (no recompute)"
        );
    }

    /// A frontier blob that vanishes between `update` (which flags it available) and
    /// the run (which reads it) must not trip the executor's "value present"
    /// invariant: `hydrate_frontier` clears the stale flag and the engine re-plans,
    /// rescheduling the node to recompute instead of pruning it behind an absent
    /// value.
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

        // get_a → sum(persist) → print(terminal). print reads sum, so sum is the
        // frontier the run must load.
        let lib = make_lib();
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a", NodeId::unique()));
        let mut sum = node(&lib, "sum", NodeId::unique());
        sum.persist = CachePersistence::Disk;
        graph.add(sum);
        graph.add(node(&lib, "print", NodeId::unique()));
        let get_a_id = graph.by_name("get_a").unwrap().id;
        let sum_id = graph.by_name("sum").unwrap().id;
        bind(&mut graph, "sum", 0, (get_a_id, 0).into());
        bind(&mut graph, "sum", 1, (get_a_id, 0).into());
        bind(&mut graph, "print", 0, (sum_id, 0).into());

        // Run 1: writes sum's blob to disk.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        engine.execute_terminals().await.unwrap();
        let after_run1 = recompute.load(Ordering::SeqCst);

        // Reopen: `update` flags sum available from its on-disk blob...
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &make_lib()).unwrap();
        // ...but the blob vanishes before the run reads it.
        for entry in std::fs::read_dir(&dir.0).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        let stats = engine.execute_terminals().await.unwrap();

        // The run completes (no panic): the failed frontier load rescheduled sum.
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
        use crate::execution::output_cache::OutputCache;
        use crate::function::{Func, FuncInput};
        use crate::library::Library;

        const PRODUCE: &str = "63b7a83c-d7fc-46f4-805a-4bf2695e3763";
        const CONSUME: &str = "39bbd6b3-b919-4095-b3d0-79a4515de75e";

        let dir = TempDir::new("wrong-type");
        let produce_runs = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(f64::NAN));

        // `produce` is a pure, Disk-persisted source; its declared output type and
        // value are `Int` when `as_float` is false, `Float` when true — same func id
        // and version, so its digest (which folds neither) is identical either way.
        // `consume` (terminal) reads it and records the value as f64.
        let build_lib =
            |as_float: bool| -> Library {
                let mut lib = Library::default();
                let produce = Func::new(PRODUCE, "produce")
                    .category("Test")
                    .pure()
                    .output(
                        "out",
                        if as_float {
                            DataType::Float
                        } else {
                            DataType::Int
                        },
                    );
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
                    .terminal()
                    .input(FuncInput::required("in", DataType::Null))
                    .lambda(async_lambda!(move |_, _, _, inputs, _, _| { recv = recv.clone() } => {
                        *recv.lock().unwrap() = inputs[0].value.as_f64().unwrap_or(f64::NAN);
                        Ok(())
                    })),
            );
                lib
            };

        let engine_with = |lib: Library| {
            let mut eg = ExecutionEngine::default();
            eg.set_output_cache(OutputCache::new(Arc::new(lib), Some(dir.0.clone())));
            eg
        };

        // produce(persist) → consume(terminal).
        let int_lib = build_lib(false);
        let mut graph = Graph::default();
        let mut produce_node = node(&int_lib, "produce", NodeId::unique());
        produce_node.persist = CachePersistence::Disk;
        let produce_id = produce_node.id;
        graph.add(produce_node);
        graph.add(node(&int_lib, "consume", NodeId::unique()));
        bind(&mut graph, "consume", 0, (produce_id, 0).into());

        // Run 1 (Int): produce runs, stores its Int blob; consume sees 7.
        let mut engine = engine_with(build_lib(false));
        engine.update(&graph, &int_lib).unwrap();
        engine.execute_terminals().await.unwrap();
        assert_eq!(produce_runs.load(Ordering::SeqCst), 1);
        assert_eq!(*received.lock().unwrap(), 7.0);

        // Run 2 (Float): the Float output re-keys produce's digest away from the Int
        // blob's key, so it isn't found — produce recomputes as Float.
        let float_lib = build_lib(true);
        let mut engine = engine_with(build_lib(true));
        engine.update(&graph, &float_lib).unwrap();
        engine.execute_terminals().await.unwrap();
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
        graph.add(node(&library, "get_b", NodeId::unique()));
        let mut mult = node(&library, "mult", NodeId::unique());
        mult.persist = CachePersistence::Disk;
        graph.add(mult);
        graph.add(node(&library, "print", NodeId::unique()));
        let get_b_id = graph.by_name("get_b").unwrap().id;
        let mult_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, (get_b_id, 0).into());
        bind(&mut graph, "mult", 1, (get_b_id, 0).into());
        bind(&mut graph, "print", 0, (mult_id, 0).into());

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &library).unwrap();
        engine.execute_terminals().await.unwrap();

        // Reopen: mult must recompute — an impure cone can't be content-addressed.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &library).unwrap();
        let stats = engine.execute_terminals().await.unwrap();
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
        graph.add(node(&library, "get_a", NodeId::unique()));
        graph.add(node(&library, "mult", NodeId::unique()));
        graph.add(node(&library, "print", NodeId::unique()));
        let get_a_id = graph.by_name("get_a").unwrap().id;
        let mult_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, (get_a_id, 0).into());
        bind(&mut graph, "mult", 1, (get_a_id, 0).into());
        bind(&mut graph, "print", 0, (mult_id, 0).into());

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &library).unwrap();
        engine.execute_terminals().await.unwrap();

        // Reopen: fresh RAM, nothing on disk for mult ⇒ it recomputes.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &library).unwrap();
        let stats = engine.execute_terminals().await.unwrap();
        assert!(
            !stats.cached_nodes.contains(&mult_id),
            "a Memory-persistence node must not be disk-cached"
        );
        assert!(
            stats.executed_nodes.iter().any(|n| n.node_id == mult_id),
            "mult recomputes on reopen"
        );
    }

    /// The codec guard on `mark_available`: a `persist` node whose blob is on disk
    /// but whose custom output type has *no registered codec* (a value written by a
    /// build that had the codec, reopened by one that doesn't) is not flagged
    /// available — it recomputes, rather than being pruned and then panicking on a
    /// failed frontier load. With the codec it's served and decodes on inspection.
    #[tokio::test]
    async fn missing_codec_skips_disk_cache_instead_of_panicking() {
        use std::any::Any;
        use std::fmt;

        use async_trait::async_trait;

        use crate::async_lambda;
        use crate::context::ContextManager;
        use crate::data::{CustomValue, TypeId};
        use crate::function::Func;
        use crate::library::{Library, TypeEntry};
        use crate::value_codec::CustomValueCodec;

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

        // A pure, terminal, disk-persisted func emitting a custom `Blob`. The type's
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
                    .terminal()
                    .output("out", DataType::Custom(BLOB_TYPE.into()))
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
            use crate::execution::output_cache::OutputCache;
            let mut engine = ExecutionEngine::default();
            engine.set_output_cache(OutputCache::new(Arc::new(library), Some(dir.0.clone())));
            engine
        };

        let dir = TempDir::new("missing-codec");
        let recompute = Arc::new(AtomicUsize::new(0));

        let mut graph = Graph::default();
        let mut blob_node = node(
            &blob_lib(true, recompute.clone()),
            "make_blob",
            NodeId::unique(),
        );
        blob_node.persist = CachePersistence::Disk;
        let blob_id = blob_node.id;
        graph.add(blob_node);

        // Run 1 (codec present): computes + writes the Blob to disk.
        let mut engine = disk_engine_with_lib(&dir, blob_lib(true, recompute.clone()));
        engine
            .update(&graph, &blob_lib(true, recompute.clone()))
            .unwrap();
        engine.execute_terminals().await.unwrap();
        assert_eq!(recompute.load(Ordering::SeqCst), 1, "cold run computes");

        // Reopen with codec: served from disk (no recompute); inspection decodes it.
        let mut engine = disk_engine_with_lib(&dir, blob_lib(true, recompute.clone()));
        engine
            .update(&graph, &blob_lib(true, recompute.clone()))
            .unwrap();
        let stats = engine.execute_terminals().await.unwrap();
        assert_eq!(
            recompute.load(Ordering::SeqCst),
            1,
            "codec present ⇒ served"
        );
        assert!(
            stats.cached_nodes.contains(&blob_id),
            "blob node disk-cached"
        );
        let vals = engine
            .get_argument_values_with_previews(&blob_id)
            .await
            .unwrap();
        assert!(
            matches!(&vals.outputs[0], DynamicValue::Custom(_)),
            "inspection decodes the blob from disk: {:?}",
            vals.outputs
        );

        // Reopen WITHOUT codec: blob present but undecodable ⇒ not flagged available
        // ⇒ recompute, no panic.
        let mut engine = disk_engine_with_lib(&dir, blob_lib(false, recompute.clone()));
        engine
            .update(&graph, &blob_lib(false, recompute.clone()))
            .unwrap();
        let stats = engine.execute_terminals().await.unwrap();
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

// === File Cache (explicit-path passthrough node) ===

mod file_cache {
    use super::*;
    use crate::graph::NodeKind;
    use crate::special::SpecialNode;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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
            "scenarium-filecache-{tag}-{}-{n}.bin",
            std::process::id()
        )))
    }

    /// `get_a` (counted source) → cache (`CachePassthrough` at `path`) → `print`
    /// (terminal). `bypass` sets the cache node's bypass toggle. The shared
    /// `get_a_calls` counter measures whether input 0 is recomputed.
    fn graph_with_cache(
        path: &Path,
        get_a_calls: Arc<AtomicUsize>,
        bypass: bool,
    ) -> (Graph, Library, NodeId, NodeId) {
        let lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || {
                get_a_calls.fetch_add(1, Ordering::SeqCst);
                Ok(7)
            }),
            ..default_hooks()
        });
        let mut graph = Graph::default();
        graph.add(node(&lib, "get_a", NodeId::unique()));
        let cache_node = Node::new(NodeKind::Special(SpecialNode::CachePassthrough { bypass }));
        let cache_id = cache_node.id;
        graph.add(cache_node);
        graph.add(node(&lib, "print", NodeId::unique()));

        let get_a_id = graph.by_name("get_a").unwrap().id;
        let print_id = graph.by_name("print").unwrap().id;
        // cache.value = get_a.0; cache.path = const; print.value = cache.0
        graph.set_input_binding(InputPort::new(cache_id, 0), (get_a_id, 0).into());
        graph.set_input_binding(
            InputPort::new(cache_id, 1),
            Binding::Const(StaticValue::FsPath(path.to_string_lossy().into_owned())),
        );
        graph.set_input_binding(InputPort::new(print_id, 0), (cache_id, 0).into());
        (graph, lib, get_a_id, cache_id)
    }

    /// The three behaviors in one flow over a shared cache file: a cold run
    /// computes + writes; a present file is served without recomputing input 0
    /// (its upstream pruned); bypass forces a recompute despite the file.
    #[tokio::test]
    async fn present_prunes_input_absent_writes_bypass_recomputes() {
        let file = temp_file("e2e");
        let path = file.0.clone();
        let calls = Arc::new(AtomicUsize::new(0));

        // Cold (file absent): get_a runs; the cache passes through + writes.
        let (graph, lib, get_a_id, _) = graph_with_cache(&path, calls.clone(), false);
        let mut engine = ExecutionEngine::default();
        engine.update(&graph, &lib).unwrap();
        let stats = engine.execute_terminals().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "cold run computes get_a");
        assert!(path.exists(), "cache file written");
        assert!(
            stats.executed_nodes.iter().any(|n| n.node_id == get_a_id),
            "get_a runs on the cold pass"
        );

        // Reopen (file present): the cache node hits, so get_a is pruned.
        let (graph, lib, get_a_id, cache_id) = graph_with_cache(&path, calls.clone(), false);
        let mut engine = ExecutionEngine::default();
        engine.update(&graph, &lib).unwrap();
        let stats = engine.execute_terminals().await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "present file ⇒ input 0 not recomputed"
        );
        assert!(
            stats.cached_nodes.contains(&cache_id),
            "cache node served from its file"
        );
        assert!(
            !stats.executed_nodes.iter().any(|n| n.node_id == get_a_id),
            "the cache node's upstream is pruned"
        );

        // Bypass on (file still present): recompute + overwrite anyway.
        let (graph, lib, _, _) = graph_with_cache(&path, calls.clone(), true);
        let mut engine = ExecutionEngine::default();
        engine.update(&graph, &lib).unwrap();
        engine.execute_terminals().await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "bypass forces a recompute even with the file present"
        );
    }
}

// === Graph Structure ===

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
            ["sum", "mult", "print"]
        );

        assert_eq!(execution_graph.program.e_nodes.len(), 5);
        assert_eq!(execution_graph.plan.process_order.len(), 5);
        assert_eq!(execution_graph.plan.execute_order.len(), 5);
        assert!(
            execution_graph
                .program
                .node_indices()
                .all(|i| !execution_graph.plan.verdicts[i].missing_required_inputs())
        );
        assert!(
            execution_graph
                .program
                .node_indices()
                .all(|i| execution_graph.plan.verdicts[i].wants_execute())
        );

        let get_a = execution_graph.by_name("get_a").unwrap();
        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        // usage_count: get_a→sum[0], get_b→sum[1]+mult[1], sum→mult[0], mult→print[0]
        assert_eq!(execution_graph.node_output_usage(get_a)[0], 1);
        assert_eq!(execution_graph.node_output_usage(get_b)[0], 2);
        assert_eq!(execution_graph.node_output_usage(sum)[0], 1);
        assert_eq!(execution_graph.node_output_usage(mult)[0], 1);

        assert!(print.terminal);

        Ok(())
    }

    #[test]
    fn updates_after_graph_change() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &library).unwrap();

        // Rewire mult to get_a and get_b directly (bypassing sum)
        let binding1: Binding = (graph.by_name("get_a").unwrap().id, 0).into();
        let binding2: Binding = (graph.by_name("get_b").unwrap().id, 0).into();
        bind(&mut graph, "mult", 0, binding1);
        bind(&mut graph, "mult", 1, binding2);

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        let get_a = execution_graph.by_name("get_a").unwrap();
        let get_b = execution_graph.by_name("get_b").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert_eq!(execution_graph.node_output_usage(get_a).len(), 1);
        assert_eq!(execution_graph.node_output_usage(get_b).len(), 1);
        assert_eq!(execution_graph.node_output_usage(mult).len(), 1);
        assert!(execution_graph.node_output_usage(print).is_empty());
        // Now each source has exactly 1 consumer (sum is no longer in the path)
        assert_eq!(execution_graph.node_output_usage(get_a)[0], 1);
        assert_eq!(execution_graph.node_output_usage(get_b)[0], 1);
        assert_eq!(execution_graph.node_output_usage(mult)[0], 1);

        Ok(())
    }

    #[test]
    fn update_rejects_func_missing_from_lib_and_keeps_prior_program() {
        let graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        // A good compile establishes a program.
        execution_graph.update(&graph, &library).unwrap();
        assert_eq!(execution_graph.program.e_nodes.len(), 5);

        // Re-compiling the same graph against a library that defines none of
        // its funcs is rejected with a message naming a missing func.
        let err = execution_graph
            .update(&graph, &Library::default())
            .unwrap_err();
        let Error::InvalidGraph { message } = err else {
            panic!("expected InvalidGraph, got {err:?}");
        };
        assert!(
            message.contains("absent from the library"),
            "message should explain the missing func, got: {message}"
        );

        // The rejection happens before any mutation, so the prior program is
        // left intact rather than torn down.
        assert_eq!(execution_graph.program.e_nodes.len(), 5);
    }

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();

        // The compiled `ExecutionProgram` is the serializable artifact; the
        // engine itself is not serializable.
        let program = &execution_graph.program;
        for format in SerdeFormat::all_formats_for_testing() {
            let serialized = common::serialize(program, format)?;
            let deserialized: ExecutionProgram = common::deserialize(&serialized, format)?;
            let serialized_again = common::serialize(&deserialized, format)?;
            assert_eq!(serialized, serialized_again);

            // Structural fields survive the round-trip (lambdas/state/output
            // values are #[serde(skip)], but ids/names/bindings must persist).
            assert_eq!(deserialized.e_nodes.len(), program.e_nodes.len());
            for original in program.e_nodes.iter() {
                let restored = deserialized.e_nodes.by_key(&original.id).unwrap();
                assert_eq!(restored.name, original.name);
                assert_eq!(restored.func_id, original.func_id);
                assert_eq!(restored.behavior, original.behavior);
                assert_eq!(restored.inputs.len, original.inputs.len);
                assert_eq!(restored.outputs.len, original.outputs.len);
            }
            // mult's Bind to sum survives with its port address intact.
            let mult = deserialized
                .e_nodes
                .iter()
                .find(|n| n.name == "mult")
                .unwrap();
            assert!(matches!(
                &deserialized.inputs[mult.inputs.range()][0].binding,
                ExecutionBinding::Bind(_)
            ));
        }

        Ok(())
    }
}

// === Missing Inputs ===

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
        let print = execution_graph.by_name("print").unwrap();

        // get_b has no missing inputs (no inputs at all)
        assert!(
            !execution_graph
                .node_verdict(get_b)
                .missing_required_inputs()
        );
        // sum is missing input[0], propagates to downstream mult and print
        assert!(execution_graph.node_verdict(sum).missing_required_inputs());
        assert!(execution_graph.node_verdict(mult).missing_required_inputs());
        assert!(
            execution_graph
                .node_verdict(print)
                .missing_required_inputs()
        );

        // Nothing should be scheduled for execution
        assert_eq!(execution_graph.plan.execute_order.len(), 0);

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
        let print = execution_graph.by_name("print").unwrap();

        // The missing flag flows through the optional bind to mult and on to print.
        assert!(execution_graph.node_verdict(sum).missing_required_inputs());
        assert!(execution_graph.node_verdict(mult).missing_required_inputs());
        assert!(
            execution_graph
                .node_verdict(print)
                .missing_required_inputs()
        );

        // The whole chain is gated — nothing executes.
        assert!(execution_node_names_in_order(&execution_graph).is_empty());

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
        let print = execution_graph.by_name("print").unwrap();

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

        let get_b_id = graph.by_name("get_b").unwrap().id;
        let sum_id = graph.by_name("sum").unwrap().id;

        // sum's required input[0] unbound → sum missing-required → gated.
        bind(&mut graph, "sum", 0, Binding::None);
        // mult[0] (required) gets a real value; mult[1] is the only bind to the
        // gated sum and is *optional* — so this exercises optional-bind
        // propagation specifically. mult and print end up gated.
        bind(&mut graph, "mult", 0, (get_b_id, 0).into());
        bind(&mut graph, "mult", 1, (sum_id, 0).into());
        library.by_name_mut("mult").unwrap().inputs[1].required = false;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        // Pre-fix, this panicked the worker; now the chain is gated and nothing runs.
        execution_graph.execute_terminals().await?;

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(execution_graph.node_verdict(mult).missing_required_inputs());
        assert!(execution_node_names_in_order(&execution_graph).is_empty());

        Ok(())
    }
}

// === Disabled Nodes ===

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

        let sum_id = graph.by_name("sum").unwrap().id;
        graph.by_id_mut(&sum_id).unwrap().disabled = true;

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
        let print = execution_graph.by_name("print").unwrap();
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

        let sum_id = graph.by_name("sum").unwrap().id;
        graph.by_id_mut(&sum_id).unwrap().disabled = true;
        library.by_name_mut("mult").unwrap().inputs[0].required = false;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        assert!(execution_graph.by_name("sum").is_none());
        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "mult", "print"]
        );

        Ok(())
    }
}

// === Const Bindings ===

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
        execution_graph.execute_terminals().await?;

        // Only mult and print execute — the const binds detach mult from its
        // upstream, so get_a/get_b/sum are pruned.
        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Re-run with the same bindings: mult's digest is unchanged, so it's a
        // RAM cache hit; only print (impure terminal) re-executes.
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;
        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        // Change one const: mult's digest changes ⇒ cache miss ⇒ it re-executes.
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(4)));
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();
        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );
        assert!(execution_graph.node_verdict(mult).wants_execute());
        assert!(!execution_graph.node_verdict(mult).is_cached());
        assert!(!execution_graph.node_verdict(mult).missing_required_inputs());
        assert!(
            !execution_graph
                .node_verdict(print)
                .missing_required_inputs()
        );

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
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Same const value: no re-execution of mult
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(3)));
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        // Different const value: mult re-executes
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(4)));
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Stable again
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

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
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "sum", "mult", "print"]
        );

        // Also unbind sum[1] — now sum has all const/none inputs, no upstream needed
        bind(&mut graph, "sum", 1, Binding::None);

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["sum", "mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn change_from_const_to_bind_recomputes() -> anyhow::Result<()> {
        let library = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        let get_b_id = graph.by_name_mut("get_b").unwrap().id;
        bind(&mut graph, "sum", 0, Binding::Const(33.into()));

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "sum", "mult", "print"]
        );

        // Switch from const back to bind — sum must re-execute
        bind(&mut graph, "sum", 0, (get_b_id, 0).into());

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["sum", "mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn optional_input_binding_change_recomputes() -> anyhow::Result<()> {
        let library = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        // Switch mult inputs to const/none
        bind(&mut graph, "mult", 0, Binding::Const(2.into()));
        bind(&mut graph, "mult", 1, Binding::None);

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Stable on rerun
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        Ok(())
    }
}

// === Behavior (Pure / Impure) ===

mod behavior {
    use super::*;

    #[test]
    fn pure_node_skips_on_rerun() -> anyhow::Result<()> {
        // `get_b` is a pure source in the fixture, so a cached output lets it skip.
        let graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        assert!(execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

        // Simulate cached output — pure node should skip
        execution_graph.set_output_values("get_b", vec![DynamicValue::Static(StaticValue::Int(7))]);

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        assert!(!execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn default_node_skips_on_rerun() -> anyhow::Result<()> {
        let graph = test_graph();
        let library = test_func_lib(default_hooks());
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "print"]
        );

        // Second run: only print (impure terminal) re-executes, others cached
        let exe_stats = execution_graph.execute_terminals().await?;
        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);
        assert_eq!(exe_stats.cached_nodes.len(), 4);

        // Cached mult must still hold the correct product, not a stale value:
        // sum = get_a(1) + get_b(11) = 12; mult = 12 * get_b(11) = 132
        let mult_id = graph.by_name("mult").unwrap().id;
        let vals = execution_graph.get_argument_values(&mult_id).unwrap();
        assert!(matches!(
            vals.outputs[0],
            DynamicValue::Static(StaticValue::Int(132))
        ));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn execute_emits_started_then_finished_progress_per_node() -> anyhow::Result<()> {
        use crate::execution_stats::{RunPhase, RunProgress};
        use tokio::sync::mpsc::unbounded_channel;

        let graph = test_graph();
        let library = test_func_lib(default_hooks());
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        let (tx, mut rx) = unbounded_channel::<RunProgress>();
        let stats = eg
            .execute(
                RunSeeds {
                    terminals: true,
                    ..Default::default()
                },
                Some(&tx),
                CancelToken::never(),
            )
            .await?;
        drop(tx);

        let mut events: Vec<(NodeId, RunPhase)> = Vec::new();
        while let Ok(p) = rx.try_recv() {
            // No subgraphs in `test_graph` → each event maps to exactly one node.
            assert_eq!(p.nodes.len(), 1);
            events.push((p.nodes[0], p.phase));
        }

        let name_of: std::collections::HashMap<NodeId, String> =
            ["get_a", "get_b", "sum", "mult", "print"]
                .iter()
                .map(|n| (graph.by_name(n).unwrap().id, n.to_string()))
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
                    terminals: true,
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
                    terminals: true,
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
        use crate::execution_stats::NodeError;
        use crate::function::Func;
        use crate::graph::{Graph, NodeId};
        use crate::library::Library;

        // Trips the cancel on its first invoke only, so the re-run completes.
        let cancel_first = Arc::new(AtomicBool::new(true));
        let library: Library = [Func::new("8400cb3a-a5d2-4fcd-a9d8-0ab4880c710f", "self_cancel")
            .category("Debug")
            .pure()
            .terminal()
            .output("out", DataType::Int)
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
        let mut node: Node = library.by_name("self_cancel").unwrap().into();
        node.id = node_id;
        graph.add(node);
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // Run 1: the node trips the cancel mid-invoke — it must not appear as
        // executed (it didn't complete), and the run is flagged cancelled.
        let stats = eg
            .execute(
                RunSeeds {
                    terminals: true,
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
                    terminals: true,
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
        use crate::execution_stats::NodeError;
        use crate::func_lambda::InvokeError;
        use crate::function::Func;
        use crate::graph::{Graph, NodeId};
        use crate::library::Library;

        let library: Library = [
            Func::new("8003e30b-0417-474d-a77f-1d3ea71ac6b3", "always_cancel")
                .category("Debug")
                .pure()
                .terminal()
                .output("out", DataType::Int)
                .lambda(async_lambda!(move |_, _, _, _, _, _| {
                    Err(InvokeError::Cancelled)
                })),
        ]
        .into();

        let mut graph = Graph::default();
        let node_id: NodeId = "c791f8aa-3bf9-435d-8530-f3904b4b6a28".into();
        let mut node: Node = library.by_name("always_cancel").unwrap().into();
        node.id = node_id;
        graph.add(node);
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        let stats = eg
            .execute(
                RunSeeds {
                    terminals: true,
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
            ["sum", "mult", "print"]
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
        execution_graph.execute_terminals().await?;

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

// === Composite (Subgraph) Caching ===

mod composite_behavior {
    use super::*;
    use crate::function::FuncOutput;
    use crate::graph::NodeKind;
    use crate::subgraph::{SubgraphDef, SubgraphRef};

    fn func_node(library: &Library, func_name: &str, node_name: &str) -> Node {
        let id = library.by_name(func_name).unwrap().id;
        let mut n = Node::new(NodeKind::Func(id));
        n.name = node_name.to_string();
        n
    }

    fn int_output(name: &str) -> FuncOutput {
        FuncOutput::new(name, DataType::Int)
    }

    /// A subgraph def with no inputs and one output, whose interior is the
    /// impure `get_b` (named `inner_name`) feeding `SubgraphOutput[0]`.
    fn impure_output_def(library: &Library, id: &str, name: &str, inner_name: &str) -> SubgraphDef {
        let inner = func_node(library, "get_b", inner_name);
        let inner_id = inner.id;
        let so = Node::new(NodeKind::SubgraphOutput);
        let so_id = so.id;
        let mut interior = Graph::default();
        interior.add(inner);
        interior.add(so);
        interior.set_input_binding(InputPort::new(so_id, 0), (inner_id, 0).into());
        SubgraphDef::new(id, name)
            .graph(interior)
            .output(int_output("Out"))
    }

    /// Main graph: one instance of `def` whose output feeds a terminal `print`.
    fn main_with(library: &Library, def: SubgraphDef) -> Graph {
        let def_id = def.id;
        let mut graph = Graph::default();
        graph.subgraphs.add(def.clone());
        let inst = graph.add_subgraph_node(&def, SubgraphRef::Local(def_id));
        let p = func_node(library, "print", "p");
        let p_id = p.id;
        graph.add(p);
        graph.set_input_binding(InputPort::new(p_id, 0), (inst, 0).into());
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
        let def = impure_output_def(
            &library,
            "00000000-0000-0000-0000-0000000000a1",
            "S",
            "inner",
        );
        let graph = main_with(&library, def);
        assert!(
            reruns_with_cache(&graph, &library, "inner"),
            "impure interior recomputes through a composite"
        );
    }

    #[test]
    fn update_rejects_func_missing_inside_subgraph() {
        // The check descends composites: a func only the *interior*
        // references, absent from the lib, is still caught.
        let library = test_func_lib(TestFuncHooks::default());
        let def = impure_output_def(
            &library,
            "00000000-0000-0000-0000-0000000000a1",
            "S",
            "inner",
        );
        let graph = main_with(&library, def);

        // A `Local` def resolves from the graph itself, so the walk reaches
        // the interior even with an empty library — and flags its `get_b`.
        let mut eg = ExecutionEngine::default();
        let err = eg.update(&graph, &Library::default()).unwrap_err();
        let Error::InvalidGraph { message } = err else {
            panic!("expected InvalidGraph, got {err:?}");
        };
        let get_b = library.by_name("get_b").unwrap().id;
        assert!(
            message.contains(&format!("{get_b:?}")),
            "message should name the interior's missing func, got: {message}"
        );
    }

    #[test]
    fn nested_impure_interior_reruns() {
        // A doubly-nested impure node recomputes — flattening preserves its
        // impurity through two composite levels.
        let mut library = test_func_lib(TestFuncHooks::default());
        library.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;
        let inner_def = impure_output_def(
            &library,
            "00000000-0000-0000-0000-0000000000b1",
            "Inner",
            "deep",
        );
        let inner_id = inner_def.id;
        let mut outer_interior = Graph::default();
        outer_interior.subgraphs.add(inner_def.clone());
        let inner_inst = outer_interior.add_subgraph_node(&inner_def, SubgraphRef::Local(inner_id));
        let so = Node::new(NodeKind::SubgraphOutput);
        let so_id = so.id;
        outer_interior.add(so);
        outer_interior.set_input_binding(InputPort::new(so_id, 0), (inner_inst, 0).into());
        let outer_def = SubgraphDef::new("00000000-0000-0000-0000-0000000000b2", "Outer")
            .graph(outer_interior)
            .output(int_output("Out"));
        let graph = main_with(&library, outer_def);
        assert!(
            reruns_with_cache(&graph, &library, "deep"),
            "doubly-nested impure interior recomputes"
        );
    }
}

// === Cycle Detection ===

mod cycle_detection {
    use super::*;

    #[test]
    fn returns_error_with_node_id() {
        let mut graph = test_graph();
        let library = test_func_lib(TestFuncHooks::default());

        // Create cycle: sum[0] ← mult (mult already depends on sum)
        let mult_node_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "sum", 0, (mult_node_id, 0).into());

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

// === Invalidation & State Reset ===

mod invalidation {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn clear_resets_graph() -> anyhow::Result<()> {
        let graph = test_graph();
        let library = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert!(!execution_graph.program.e_nodes.is_empty());

        execution_graph.clear();

        assert!(execution_graph.program.e_nodes.is_empty());
        assert!(execution_graph.plan.process_order.is_empty());
        assert!(execution_graph.plan.execute_order.is_empty());
        // The SoA pools are emptied too (not just the node list).
        assert!(execution_graph.program.inputs.is_empty());
        assert_eq!(execution_graph.program.n_outputs, 0);
        assert!(execution_graph.program.events.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reset_states_clears_outputs() -> anyhow::Result<()> {
        let graph = test_graph();
        let library = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        // Verify outputs exist before reset
        let sum = execution_graph.by_name("sum").unwrap();
        assert!(execution_graph.runtime_slot(sum).output_values().is_some());

        execution_graph.reset_states();

        // All output_values and state should be cleared
        for (e_node, slot) in execution_graph
            .program
            .e_nodes
            .iter()
            .zip(execution_graph.runtime_slots())
        {
            assert!(
                slot.output_values().is_none(),
                "node {} should have no output_values",
                e_node.name
            );
            assert!(
                slot.state.is_none(),
                "node {} should have no state",
                e_node.name
            );
            assert!(
                slot.event_state.lock().await.is_none(),
                "node {} should have no event state",
                e_node.name
            );
        }

        Ok(())
    }
}

// === Full Execution ===

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
        execution_graph.execute_terminals().await?;
        // sum = get_a + get_b = 2 + 5 = 7, mult = sum * get_b = 7 * 5 = 35
        assert_eq!(test_values.try_lock()?.result, 35);

        // Changing external state doesn't recompute: get_b is pure, so its digest
        // is stable and the cached value stands.
        test_values.try_lock()?.b = 7;

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;
        assert_eq!(test_values.try_lock()?.result, 35);

        // Make get_b Impure: now it re-reads the value
        library.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;
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

        execution_graph.execute_terminals().await?;
        let order1 = execution_node_names_in_order(&execution_graph);

        execution_graph.execute_terminals().await?;
        let order2 = execution_node_names_in_order(&execution_graph);

        // Execution order should be stable across runs
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

        eg.execute_terminals().await?;
        let run1 = execution_node_names_in_order(&eg);
        eg.execute_terminals().await?;
        let run2 = execution_node_names_in_order(&eg);
        eg.execute_terminals().await?;
        let run3 = execution_node_names_in_order(&eg);

        // First run executes everything; once the pure upstream is cached, runs 2
        // and 3 must schedule identically — guards the reused `Scratch` buffers
        // being reset cleanly each run (a missed reset would drift).
        assert_eq!(run2, ["print"]);
        assert_eq!(run2, run3);
        assert_ne!(run1, run2);

        // The cached product stays correct every run: sum(1+11=12) * get_b(11) = 132.
        let mult_id = graph.by_name("mult").unwrap().id;
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
        execution_graph.execute_terminals().await?;

        // Switch mult to const inputs
        bind(&mut graph, "mult", 0, Binding::Const(2.into()));
        bind(&mut graph, "mult", 1, Binding::Const(21.into()));

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Switch back to bind from cached get_b — mult re-executes with cached upstream
        let get_b_id = graph.by_name_mut("get_b").unwrap().id;
        bind(&mut graph, "mult", 0, (get_b_id, 0).into());

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        Ok(())
    }

    /// Output buffers are **not** wiped between runs — a re-running node reuses its
    /// slot's buffer in place, so an output port a lambda leaves unwritten this run
    /// retains its prior value. This is the contract the reuse model rests on
    /// (a skipped node keeps its whole prior output); the flip side is that a lambda
    /// must write *all* the outputs it means to produce each run. The node is impure
    /// (re-runs each time): run 1 writes both ports, run 2 writes only port 0, so
    /// port 1 keeps the `20` from run 1.
    #[tokio::test(flavor = "multi_thread")]
    async fn unwritten_output_port_retains_prior_value() -> anyhow::Result<()> {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use crate::async_lambda;
        use crate::function::Func;
        use crate::graph::Graph;
        use crate::library::Library;

        let invocations = Arc::new(AtomicUsize::new(0));
        let library: Library = [Func::new(
            "4df6d99f-cb0c-479c-9b94-6549c406d9ab",
            "partial_writer",
        )
        .category("Debug")
        .terminal()
        .output("a", DataType::Int)
        .output("b", DataType::Int)
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
        node.id = "0b35e5e4-be30-4733-a5a2-9d474000de10".into();
        graph.add(node);
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // Run 1: both ports written.
        eg.execute_terminals().await?;
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

        // Run 2: only port 0 is written. With no pre-run wipe the lambda reuses the
        // slot's buffer in place, so port 1 keeps run 1's `20`.
        eg.execute_terminals().await?;
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
            matches!(outputs[1], DynamicValue::Static(StaticValue::Int(20))),
            "the unwritten port retains its prior value (no wipe): {outputs:?}"
        );

        Ok(())
    }
}

// === Argument Values ===

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
        let mult_id = graph.by_name("mult").unwrap().id;

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

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
        execution_graph.execute_terminals().await?;

        // sum: inputs are get_a(2.0) and get_b(5.0), output is 2+5=7
        let sum_id = graph.by_name("sum").unwrap().id;
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
        let mult_id = graph.by_name("mult").unwrap().id;
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
        let print_id = graph.by_name("print").unwrap().id;
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
        let mult_id = graph.by_name("mult").unwrap().id;

        execution_graph.update(&graph, &library).unwrap();
        execution_graph.execute_terminals().await?;

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

        let sum_id = graph.by_name("sum").unwrap().id;
        let values = execution_graph.get_argument_values(&sum_id).unwrap();

        // Before execution: all inputs are None (no upstream values yet)
        assert_eq!(values.inputs.len(), 2);
        assert!(values.inputs[0].is_none());
        assert!(values.inputs[1].is_none());
        assert!(values.outputs.is_empty());

        Ok(())
    }
}

// === Error Propagation ===

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

        let stats = execution_graph.execute_terminals().await?;

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
        for name in ["sum", "mult", "print"] {
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

// === Execution Stats ===

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
        let stats = execution_graph.execute_terminals().await?;

        // sum[0] should appear in missing_inputs
        let sum_id = graph.by_name("sum").unwrap().id;
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
        let stats = execution_graph.execute_terminals().await?;

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
        let sum_id = graph.by_name("sum").unwrap().id;
        let print_id = graph.by_name("print").unwrap().id;
        assert!(stats.executed_nodes.iter().any(|n| n.node_id == sum_id));
        assert!(stats.executed_nodes.iter().any(|n| n.node_id == print_id));

        // No errors on first clean run
        assert!(stats.node_errors.is_empty());
        assert!(stats.missing_inputs.is_empty());

        Ok(())
    }
}

// === Events ===

mod events {
    use super::*;
    use crate::async_lambda;
    use crate::event_lambda::EventLambda;
    use crate::execution::event::EventRef;
    use crate::function::{Func, FuncInput};

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
    // terminal, so only event-driven execution reaches them.
    fn build() -> EventFixture {
        let emit_calls = Arc::new(Mutex::new(0));
        let recv_values = Arc::new(Mutex::new(Vec::new()));
        let emit_calls_l = emit_calls.clone();
        let recv_values_l = recv_values.clone();

        // Both funcs are Impure non-terminals (the `Func::new` default).
        let mut library = Library::default();
        library.add(
            Func::new(EMIT_FUNC, "emit")
                .output("out", DataType::Int)
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
        graph.add(node(&library, "emit", emit_id));
        graph.add(node(&library, "recv", recv_id));
        graph.subscribe(emit_id, 0, recv_id);
        graph.set_input_binding(InputPort::new(recv_id, 0), (emit_id, 0).into());
        graph.validate();

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

        // terminals=false, event_triggers=true → emit (owns a subscribed event)
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
        // Drop the subscriber but keep emit reachable by making it a terminal.
        let emit_id = f.emit_id;
        let recv_id = f.graph.by_name("recv").unwrap().id;
        f.graph.unsubscribe(emit_id, 0, recv_id);
        f.library.by_name_mut("emit").unwrap().terminal = true;

        let mut eg = ExecutionEngine::default();
        eg.update(&f.graph, &f.library).unwrap();
        let stats = eg.execute_terminals().await?;

        // emit ran, but its event has no subscribers → no live triggers.
        assert!(stats.executed_nodes.iter().any(|n| n.node_id == f.emit_id));
        assert!(eg.active_event_triggers(&stats).is_empty());

        Ok(())
    }
}

// === Output Usage (Skip / Needed) ===

mod output_usage {
    use super::*;
    use crate::func_lambda::OutputUsage;
    use crate::{
        async_lambda,
        function::{Func, FuncInput},
    };

    const SPLIT_FUNC: FuncId = FuncId::from_u128(0x5911);
    const SINK_FUNC: FuncId = FuncId::from_u128(0x5922);

    #[tokio::test(flavor = "multi_thread")]
    async fn unused_output_marked_skip() -> anyhow::Result<()> {
        let seen_usage: Arc<Mutex<Vec<OutputUsage>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_usage_l = seen_usage.clone();

        let mut library = Library::default();
        library.add(
            Func::new(SPLIT_FUNC, "split")
                .output("a", DataType::Int)
                .output("b", DataType::Int)
                .lambda(async_lambda!(
                    move |_, _, _, _, usage, outputs| { seen = seen_usage_l.clone() } => {
                        seen.lock().await.extend_from_slice(usage);
                        outputs[0] = DynamicValue::Static(StaticValue::Int(1));
                        outputs[1] = DynamicValue::Static(StaticValue::Int(2));
                        Ok(())
                    }
                )),
        );
        library.add(
            Func::new(SINK_FUNC, "sink")
                .terminal()
                .input(FuncInput::required("in", DataType::Int))
                .lambda(async_lambda!(|_, _, _, _, _, _| { Ok(()) })),
        );

        let split_id = NodeId::unique();
        let sink_id = NodeId::unique();
        let mut graph = Graph::default();
        graph.add(node(&library, "split", split_id));
        graph.add(node(&library, "sink", sink_id));
        // Consume only output 0; output 1 has no consumer.
        graph.set_input_binding(InputPort::new(sink_id, 0), (split_id, 0).into());
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_terminals().await?;

        let split = eg.by_name("split").unwrap();
        assert_eq!(eg.node_output_usage(split)[0], 1);
        assert_eq!(eg.node_output_usage(split)[1], 0);

        // The lambda observed Needed for the consumed output, Skip for the other.
        assert_eq!(
            *seen_usage.lock().await,
            vec![OutputUsage::Needed(1), OutputUsage::Skip]
        );

        Ok(())
    }
}

// === Topology Edge Cases ===

mod topology {
    use super::*;
    use common::FloatExt;

    #[tokio::test(flavor = "multi_thread")]
    async fn removing_node_compacts_and_remaps() -> anyhow::Result<()> {
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
        assert_eq!(eg.program.e_nodes.len(), 5);

        // Remove get_b — a middle node feeding sum[1] and mult[1] (both optional).
        // Forces compaction and target_idx remapping for the survivors.
        let get_b_id = graph.by_name("get_b").unwrap().id;
        graph.remove_by_id(get_b_id);
        graph.validate();

        eg.update(&graph, &library).unwrap();
        assert_eq!(eg.program.e_nodes.len(), 4);
        assert!(eg.by_name("get_b").is_none());

        eg.execute_terminals().await?;

        // sum = get_a(2) + none(0) = 2; mult = sum(2) * none(default 1) = 2
        assert_eq!(*printed.lock().await, 2);

        // sum's Bind to get_a still resolves after the index remap.
        let sum_id = graph.by_name("sum").unwrap().id;
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

        let stats = eg.execute_terminals().await?;
        assert!(stats.executed_nodes.is_empty());
        assert!(stats.node_errors.is_empty());
        assert!(stats.missing_inputs.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn multiple_terminals_all_execute() -> anyhow::Result<()> {
        let printed = Arc::new(Mutex::new(Vec::<i64>::new()));
        let printed_l = printed.clone();
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 5),
            print: Arc::new(move |v| printed_l.try_lock().unwrap().push(v)),
        });

        // Two independent terminal chains: get_a→print1, get_b→print2.
        let get_a_id = NodeId::unique();
        let get_b_id = NodeId::unique();
        let print1_id = NodeId::unique();
        let print2_id = NodeId::unique();

        let mut graph = Graph::default();
        graph.add(node(&library, "get_a", get_a_id));
        graph.add(node(&library, "get_b", get_b_id));
        graph.add(node(&library, "print", print1_id));
        graph.add(node(&library, "print", print2_id));
        graph.set_input_binding(InputPort::new(print1_id, 0), (get_a_id, 0).into());
        graph.set_input_binding(InputPort::new(print2_id, 0), (get_b_id, 0).into());
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        let stats = eg.execute_terminals().await?;

        // Both terminals plus both sources execute exactly once.
        assert_eq!(stats.executed_nodes.len(), 4);
        let mut got = printed.lock().await.clone();
        got.sort();
        assert_eq!(got, vec![2, 5]);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_output_survives_compaction_reorder() -> anyhow::Result<()> {
        // get_a is Pure → its output is cached across runs. We remove an
        // earlier-inserted chain so compaction shifts get_a's slot, then verify
        // its cached output (carried on the node) still resolves and it is NOT
        // recomputed. Guards the SoA span / `output_values` carry-over across a
        // `compact_insert` reorder.
        let calls_a = Arc::new(Mutex::new(0));
        let calls_a_l = calls_a.clone();
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || {
                *calls_a_l.try_lock().unwrap() += 1;
                Ok(2)
            }),
            get_b: Arc::new(|| 5),
            print: Arc::new(|_| {}),
        });

        // get_a's chain is inserted AFTER get_b's, so removing get_b's chain
        // shifts get_a to a lower exec index on the next update.
        let get_b_id = NodeId::unique();
        let print_b_id = NodeId::unique();
        let get_a_id = NodeId::unique();
        let print_a_id = NodeId::unique();

        let mut graph = Graph::default();
        graph.add(node(&library, "get_b", get_b_id));
        graph.add(node(&library, "print", print_b_id));
        graph.add(node(&library, "get_a", get_a_id));
        graph.add(node(&library, "print", print_a_id));
        graph.set_input_binding(InputPort::new(print_b_id, 0), (get_b_id, 0).into());
        graph.set_input_binding(InputPort::new(print_a_id, 0), (get_a_id, 0).into());
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_terminals().await?;
        assert_eq!(*calls_a.lock().await, 1); // get_a ran once
        let idx_before = eg.program.e_nodes.index_of_key(&get_a_id).unwrap();

        // Remove get_b's chain — get_a's slot compacts toward the front.
        graph.remove_by_id(get_b_id);
        graph.remove_by_id(print_b_id);
        graph.validate();

        eg.update(&graph, &library).unwrap();
        let idx_after = eg.program.e_nodes.index_of_key(&get_a_id).unwrap();
        assert_ne!(idx_before, idx_after, "get_a should have been reordered");

        let stats = eg.execute_terminals().await?;

        // get_a (Pure) stays cached, not re-executed, despite the reorder…
        assert_eq!(*calls_a.lock().await, 1, "get_a recomputed after reorder");
        assert!(stats.cached_nodes.contains(&get_a_id));
        // …and its cached output still resolves to the correct value.
        let vals = eg.get_argument_values(&get_a_id).unwrap();
        assert!(
            matches!(vals.outputs[0], DynamicValue::Static(StaticValue::Float(v)) if v.approximately_eq(2.0))
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn repeated_structural_churn_stays_correct() -> anyhow::Result<()> {
        // Grow→shrink the graph repeatedly on ONE ExecutionEngine, re-executing
        // each step. Stresses the SoA pool rebuild + compaction across many
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
        graph.add(node(&library, "get_a", get_a_id));
        graph.add(node(&library, "print", print_a_id));
        graph.set_input_binding(InputPort::new(print_a_id, 0), (get_a_id, 0).into());
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_terminals().await?;

        for round in 0..3 {
            // Add a get_b → print chain.
            let gb = NodeId::unique();
            let pb = NodeId::unique();
            graph.add(node(&library, "get_b", gb));
            graph.add(node(&library, "print", pb));
            graph.set_input_binding(InputPort::new(pb, 0), (gb, 0).into());
            graph.validate();
            eg.update(&graph, &library).unwrap();
            assert_eq!(eg.program.e_nodes.len(), 4, "round {round} grow");
            printed.lock().await.clear();
            eg.execute_terminals().await?;
            let mut got = printed.lock().await.clone();
            got.sort();
            assert_eq!(got, vec![2, 5], "round {round} grow values");

            // Remove it again.
            graph.remove_by_id(gb);
            graph.remove_by_id(pb);
            graph.validate();
            eg.update(&graph, &library).unwrap();
            assert_eq!(eg.program.e_nodes.len(), 2, "round {round} shrink");
            printed.lock().await.clear();
            eg.execute_terminals().await?;
            assert_eq!(
                *printed.lock().await,
                vec![2],
                "round {round} shrink values"
            );
        }

        Ok(())
    }
}

// === Previews ===

mod previews {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn previews_match_plain_argument_values() -> anyhow::Result<()> {
        let library = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 5),
            print: Arc::new(|_| {}),
        });
        let graph = test_graph();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_terminals().await?;

        // For non-Custom values gen_preview is a no-op, so the preview variant
        // must return exactly the same values as the plain accessor.
        let mult_id = graph.by_name("mult").unwrap().id;
        let plain = eg.get_argument_values(&mult_id).unwrap();
        let with_previews = eg
            .get_argument_values_with_previews(&mult_id)
            .await
            .unwrap();

        // mult = sum(2+5=7) * get_b(5) = 35
        assert!(matches!(
            with_previews.outputs[0],
            DynamicValue::Static(StaticValue::Int(35))
        ));
        assert_eq!(plain.inputs.len(), with_previews.inputs.len());
        assert_eq!(plain.outputs.len(), with_previews.outputs.len());
        assert!(matches!(
            with_previews.inputs[0],
            Some(DynamicValue::Static(StaticValue::Int(7)))
        ));

        Ok(())
    }
}

// === Subgraph Flattening (Stage 2) ===

mod subgraph {
    use super::*;
    use crate::event_lambda::EventLambda;
    use crate::function::{Func, FuncId, FuncInput, FuncOutput};
    use crate::graph::NodeKind;
    use crate::subgraph::{SubgraphDef, SubgraphEvent, SubgraphId, SubgraphRef};
    use std::sync::Mutex as StdMutex;

    fn fnode(library: &Library, name: &str) -> Node {
        library.by_name(name).unwrap().into()
    }

    fn int_out(name: &str) -> FuncOutput {
        FuncOutput::new(name, DataType::Int)
    }

    /// `in(A,B) -> sum -> out(Sum)`.
    fn wrap_sum_def(library: &Library) -> SubgraphDef {
        let in_node = Node::new(NodeKind::SubgraphInput);
        let in_id = in_node.id;
        let sum = fnode(library, "sum");
        let sum_id = sum.id;
        let out = Node::new(NodeKind::SubgraphOutput);
        let out_id = out.id;

        let mut graph = Graph::default();
        graph.add(in_node);
        graph.add(sum);
        graph.add(out);
        graph.set_input_binding(InputPort::new(sum_id, 0), (in_id, 0).into());
        graph.set_input_binding(InputPort::new(sum_id, 1), (in_id, 1).into());
        graph.set_input_binding(InputPort::new(out_id, 0), (sum_id, 0).into());

        SubgraphDef::new(SubgraphId::unique(), "WrapSum")
            .category("Test")
            .graph(graph)
            .input(FuncInput::required("A", DataType::Int))
            .input(FuncInput::optional("B", DataType::Int))
            .output(int_out("Sum"))
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
        let (a_id, b_id) = (get_a.id, get_b.id);
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;
        let print = fnode(&library, "print");
        let print_id = print.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(get_a);
        graph.add(get_b);
        graph.add(c);
        graph.add(print);
        graph.set_input_binding(InputPort::new(c_id, 0), (a_id, 0).into());
        graph.set_input_binding(InputPort::new(c_id, 1), (b_id, 0).into());
        graph.set_input_binding(InputPort::new(print_id, 0), (c_id, 0).into());

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_terminals().await?;

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
        let (sa, sb) = (src_a.id, src_b.id);
        let out = Node::new(NodeKind::SubgraphOutput);
        let out_id = out.id;
        let mut def_graph = Graph::default();
        def_graph.add(src_a);
        def_graph.add(src_b);
        def_graph.add(out);
        def_graph.set_input_binding(InputPort::new(out_id, 0), (sa, 0).into());
        def_graph.set_input_binding(InputPort::new(out_id, 1), (sb, 0).into());
        let def = SubgraphDef::new(SubgraphId::unique(), "TwoSources")
            .category("Test")
            .graph(def_graph)
            .outputs([int_out("O0"), int_out("O1")]);

        // parent: C, print <- C.out0 (out1 unused).
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;
        let print = fnode(&library, "print");
        let print_id = print.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(c);
        graph.add(print);
        graph.set_input_binding(InputPort::new(print_id, 0), (c_id, 0).into());

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_terminals().await?;

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
        // so the cyclic node is reachable from a terminal.
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;
        let print = fnode(&library, "print");
        let print_id = print.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(c);
        graph.add(print);
        graph.set_input_binding(InputPort::new(c_id, 0), (c_id, 0).into());
        graph.set_input_binding(InputPort::new(print_id, 0), (c_id, 0).into());

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        let result = eg.execute_terminals().await;

        assert!(
            matches!(result, Err(Error::CycleDetected { .. })),
            "expected CycleDetected, got {result:?}"
        );
    }

    fn bind_target(eg: &ExecutionEngine, e: &ExecutionNode, input_idx: usize) -> NodeId {
        match &eg.node_inputs(e)[input_idx].binding {
            ExecutionBinding::Bind(addr) => eg.program.e_nodes[addr.target_idx].id,
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
        let (a_id, b_id) = (get_a.id, get_b.id);
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;
        let print = fnode(&library, "print");
        let print_id = print.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(get_a);
        graph.add(get_b);
        graph.add(c);
        graph.add(print);
        graph.set_input_binding(InputPort::new(c_id, 0), (a_id, 0).into());
        graph.set_input_binding(InputPort::new(c_id, 1), (b_id, 0).into());
        graph.set_input_binding(InputPort::new(print_id, 0), (c_id, 0).into());

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // get_a, get_b, sum (interior), print — no composite/boundary nodes.
        assert_eq!(eg.program.e_nodes.len(), 4);
        let sum = eg.by_name("sum").unwrap();
        assert_eq!(bind_target(&eg, sum, 0), a_id);
        assert_eq!(bind_target(&eg, sum, 1), b_id);
        assert_eq!(bind_target(&eg, eg.by_name("print").unwrap(), 0), sum.id);
    }

    /// A func-only graph builds with the node ids unchanged (caches survive).
    #[test]
    fn top_level_func_nodes_keep_identity() {
        let library = test_func_lib(default_hooks());
        let graph = test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        assert_eq!(eg.program.e_nodes.len(), graph.len());
        for node in graph.iter() {
            assert!(eg.by_id(&node.id).is_some(), "id preserved");
        }
    }

    /// Two instances of one def produce two distinct interior leaves.
    #[test]
    fn two_instances_get_distinct_leaf_ids() {
        let mut library = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum_def(&library);
        let def_id = def.id;
        library.add_subgraph(def);

        let mut graph = Graph::default();
        let def_ref = library.subgraph_by_id(&def_id).unwrap();
        graph.add(Node::subgraph_instance(
            def_ref,
            SubgraphRef::Linked(def_id),
        ));
        graph.add(Node::subgraph_instance(
            def_ref,
            SubgraphRef::Linked(def_id),
        ));

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        let sums: Vec<NodeId> = eg
            .program
            .e_nodes
            .iter()
            .filter(|e| e.name == "sum")
            .map(|e| e.id)
            .collect();
        assert_eq!(sums.len(), 2);
        assert_ne!(sums[0], sums[1]);
    }

    /// The `FlattenMap` maps a flattened interior node back to the
    /// editor's authoring ids: `attribution` yields the node's own id
    /// inside the def's graph, then each enclosing composite instance.
    /// This is what lets the editor show per-node stats inside a subgraph
    /// and accumulate them onto the instance node.
    #[test]
    fn flatten_map_attributes_interior_to_authoring_ids() {
        let library = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum_def(&library);
        // The id the editor knows the interior node by (in the def graph).
        let interior_sum_id = def.graph.iter().find(|n| n.name == "sum").unwrap().id;

        let get_a = fnode(&library, "get_a");
        let a_id = get_a.id;
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(get_a);
        graph.add(c);
        graph.set_input_binding(InputPort::new(c_id, 0), (a_id, 0).into());

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // Interior node: flattened id is remapped, but attribution points
        // back to the authoring interior id then the enclosing instance.
        let sum_flat = eg.by_name("sum").unwrap().id;
        assert_ne!(sum_flat, interior_sum_id, "flattened id is remapped");
        let attr: Vec<_> = eg.flatten_map.attribution(sum_flat).collect();
        assert_eq!(attr, vec![interior_sum_id, c_id]);

        // Top-level node: id unchanged, attribution is just itself.
        let a_attr: Vec<_> = eg.flatten_map.attribution(a_id).collect();
        assert_eq!(a_attr, vec![a_id]);
    }

    // === Stage 2b: events across boundaries ===

    /// Add a `ticker` func (one event, no I/O) usable as an interior or parent
    /// emitter; instantiate it by name with `fnode`.
    fn add_ticker(library: &mut Library) {
        library.add(
            Func::new(FuncId::unique(), "ticker")
                .category("Test")
                .terminal()
                .event("tick", EventLambda::default()),
        );
    }

    fn func_lib_with_ticker() -> Library {
        let mut library = test_func_lib(default_hooks());
        add_ticker(&mut library);
        library
    }

    fn subscriber_ids(eg: &ExecutionEngine, e: &ExecutionNode, event_idx: usize) -> Vec<NodeId> {
        eg.node_events(e)[event_idx]
            .subscribers
            .iter()
            .map(|&i| eg.program.e_nodes[i].id)
            .collect()
    }

    /// A parent subscriber of a composite's exposed event is rewired onto the
    /// flattened interior emitter.
    #[test]
    fn exposed_event_rewires_parent_subscriber_to_interior_emitter() {
        let library = func_lib_with_ticker();

        // def: a single `ticker`, its `tick` event exposed as the composite's
        // event 0.
        let emitter = fnode(&library, "ticker");
        let emitter_id = emitter.id;
        let mut def_graph = Graph::default();
        def_graph.add(emitter);
        let def = SubgraphDef::new(SubgraphId::unique(), "Exposer")
            .category("Test")
            .graph(def_graph)
            .event(SubgraphEvent {
                name: "tick".into(),
                emitter: emitter_id,
                emitter_event_idx: 0,
            });

        // parent: composite C, and `listener` subscribing to C's event 0.
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;
        let listener = fnode(&library, "print");
        let listener_id = listener.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(c);
        graph.add(listener);
        graph.subscribe(c_id, 0, listener_id);

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // The flattened interior `ticker` carries the rewired subscriber.
        let ticker_node = eg.by_name("ticker").unwrap();
        assert_eq!(subscriber_ids(&eg, ticker_node, 0), vec![listener_id]);
    }

    /// Triggering a composite (as a subscriber) reaches the interior nodes
    /// wired to its `SubgraphInput` trigger.
    #[test]
    fn triggering_composite_reaches_interior_subscribers() {
        let library = func_lib_with_ticker();

        // def: SubgraphInput trigger → interior `print` subscribes to it.
        let si = Node::new(NodeKind::SubgraphInput);
        let si_id = si.id;
        let reactor = fnode(&library, "print");
        let reactor_id = reactor.id;
        let mut def_graph = Graph::default();
        def_graph.add(si);
        def_graph.add(reactor);
        def_graph.subscribe(si_id, 0, reactor_id);
        let def = SubgraphDef::new(SubgraphId::unique(), "Reactor")
            .category("Test")
            .graph(def_graph);

        // parent: `ticker` emits; composite C subscribes to it.
        let emitter = fnode(&library, "ticker");
        let emitter_id = emitter.id;
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(emitter);
        graph.add(c);
        graph.subscribe(emitter_id, 0, c_id);

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();

        // The interior `print` flat id is the one wired onto `ticker`'s event.
        let reactor_flat = eg.by_name("print").unwrap().id;
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
        let def_id = def.id;
        library.add_subgraph(def);

        // Two linked instances with const inputs, each feeding a print.
        let def_ref = library.subgraph_by_id(&def_id).unwrap();
        let c1 = Node::subgraph_instance(def_ref, SubgraphRef::Linked(def_id));
        let c2 = Node::subgraph_instance(def_ref, SubgraphRef::Linked(def_id));
        let (c1_id, c2_id) = (c1.id, c2.id);
        let p1 = fnode(&library, "print");
        let p2 = fnode(&library, "print");
        let (p1_id, p2_id) = (p1.id, p2.id);

        let mut graph = Graph::default();
        graph.add(c1);
        graph.add(c2);
        graph.add(p1);
        graph.add(p2);
        graph.set_input_binding(InputPort::new(c1_id, 0), StaticValue::Int(1).into());
        graph.set_input_binding(InputPort::new(c1_id, 1), StaticValue::Int(2).into());
        graph.set_input_binding(InputPort::new(c2_id, 0), StaticValue::Int(10).into());
        graph.set_input_binding(InputPort::new(c2_id, 1), StaticValue::Int(20).into());
        graph.set_input_binding(InputPort::new(p1_id, 0), (c1_id, 0).into());
        graph.set_input_binding(InputPort::new(p2_id, 0), (c2_id, 0).into());

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &library).unwrap();
        eg.execute_terminals().await?;

        let mut got = captured.lock().unwrap().clone();
        got.sort();
        assert_eq!(got, vec![3, 30]); // sums: 1+2, 10+20
        captured.lock().unwrap().clear();

        // Interior `sum` flat ids (sorted) — for the cache-stability check.
        let sum_ids = |eg: &ExecutionEngine| -> Vec<NodeId> {
            let mut ids: Vec<NodeId> = eg
                .program
                .e_nodes
                .iter()
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
            let def = library.subgraphs.by_key_mut(&def_id).unwrap();
            let si = def
                .graph
                .iter()
                .find(|n| matches!(n.kind, NodeKind::SubgraphInput))
                .unwrap()
                .id;
            let so = def
                .graph
                .iter()
                .find(|n| matches!(n.kind, NodeKind::SubgraphOutput))
                .unwrap()
                .id;
            def.graph
                .set_input_binding(InputPort::new(so, 0), (si, 0).into());
        }

        eg.update(&graph, &library).unwrap();
        eg.execute_terminals().await?;

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

        // def Reactor: SubgraphInput trigger → interior `get_a` subscribes.
        let si = Node::new(NodeKind::SubgraphInput);
        let si_id = si.id;
        let reactor = fnode(&library, "get_a");
        let reactor_id = reactor.id;
        let mut def_graph = Graph::default();
        def_graph.add(si);
        def_graph.add(reactor);
        def_graph.subscribe(si_id, 0, reactor_id);
        let def = SubgraphDef::new(SubgraphId::unique(), "Reactor")
            .category("Test")
            .graph(def_graph);

        // parent: `ticker` E; composite C subscribes to E's event.
        let emitter = fnode(&library, "ticker");
        let emitter_id = emitter.id;
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(emitter);
        graph.add(c);
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
