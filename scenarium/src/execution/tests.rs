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
fn node(func_lib: &FuncLib, func_name: &str, id: NodeId) -> Node {
    let mut node: Node = func_lib.by_name(func_name).unwrap().into();
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
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

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
    /// empty registry is fine — these tests cache plain values.
    fn disk_engine(dir: &TempDir) -> ExecutionEngine {
        use crate::execution::output_cache::OutputCache;
        use crate::value_codec::CustomValueRegistry;
        let mut engine = ExecutionEngine::default();
        engine.set_output_cache(OutputCache::new(
            CustomValueRegistry::default(),
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

    /// A `persist` node whose cone contains an impure node has digest `None`, so
    /// it's never disk-cached even with `persist=Disk` — on reopen it recomputes.
    #[tokio::test]
    async fn impure_cone_persist_node_is_not_disk_cached() {
        let dir = TempDir::new("impure-cone");
        let mut func_lib = test_func_lib(default_hooks());
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        // get_b (impure) → mult (persist) → print. mult's cone is impure.
        let mut graph = Graph::default();
        graph.add(node(&func_lib, "get_b", NodeId::unique()));
        let mut mult = node(&func_lib, "mult", NodeId::unique());
        mult.persist = CachePersistence::Disk;
        graph.add(mult);
        graph.add(node(&func_lib, "print", NodeId::unique()));
        let get_b_id = graph.by_name("get_b").unwrap().id;
        let mult_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, (get_b_id, 0).into());
        bind(&mut graph, "mult", 1, (get_b_id, 0).into());
        bind(&mut graph, "print", 0, (mult_id, 0).into());

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &func_lib).unwrap();
        engine.execute_terminals().await.unwrap();

        // Reopen: mult must recompute — an impure cone can't be content-addressed.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(default_hooks());

        // get_a (pure) → mult (Memory, the default) → print.
        let mut graph = Graph::default();
        graph.add(node(&func_lib, "get_a", NodeId::unique()));
        graph.add(node(&func_lib, "mult", NodeId::unique()));
        graph.add(node(&func_lib, "print", NodeId::unique()));
        let get_a_id = graph.by_name("get_a").unwrap().id;
        let mult_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "mult", 0, (get_a_id, 0).into());
        bind(&mut graph, "mult", 1, (get_a_id, 0).into());
        bind(&mut graph, "print", 0, (mult_id, 0).into());

        let mut engine = disk_engine(&dir);
        engine.update(&graph, &func_lib).unwrap();
        engine.execute_terminals().await.unwrap();

        // Reopen: fresh RAM, nothing on disk for mult ⇒ it recomputes.
        let mut engine = disk_engine(&dir);
        engine.update(&graph, &func_lib).unwrap();
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
    ) -> (Graph, FuncLib, NodeId, NodeId) {
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
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
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
                .plan
                .node_flags
                .iter()
                .all(|f| !f.missing_required_inputs)
        );
        assert!(
            execution_graph
                .plan
                .node_flags
                .iter()
                .all(|f| f.wants_execute)
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
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &func_lib).unwrap();

        // Rewire mult to get_a and get_b directly (bypassing sum)
        let binding1: Binding = (graph.by_name("get_a").unwrap().id, 0).into();
        let binding2: Binding = (graph.by_name("get_b").unwrap().id, 0).into();
        bind(&mut graph, "mult", 0, binding1);
        bind(&mut graph, "mult", 1, binding2);

        execution_graph.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        // A good compile establishes a program.
        execution_graph.update(&graph, &func_lib).unwrap();
        assert_eq!(execution_graph.program.e_nodes.len(), 5);

        // Re-compiling the same graph against a library that defines none of
        // its funcs is rejected with a message naming a missing func.
        let err = execution_graph
            .update(&graph, &FuncLib::default())
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
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();

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
        let func_lib = test_func_lib(TestFuncHooks::default());

        // Remove sum's first input binding (required by default)
        bind(&mut graph, "sum", 0, Binding::None);

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        // get_b has no missing inputs (no inputs at all)
        assert!(!execution_graph.node_flags(get_b).missing_required_inputs);
        // sum is missing input[0], propagates to downstream mult and print
        assert!(execution_graph.node_flags(sum).missing_required_inputs);
        assert!(execution_graph.node_flags(mult).missing_required_inputs);
        assert!(execution_graph.node_flags(print).missing_required_inputs);

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
        let mut func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        // sum missing-required; mult[0] stays bound to sum but is made optional.
        bind(&mut graph, "sum", 0, Binding::None);
        func_lib.by_name_mut("mult").unwrap().inputs[0].required = false;

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        // The missing flag flows through the optional bind to mult and on to print.
        assert!(execution_graph.node_flags(sum).missing_required_inputs);
        assert!(execution_graph.node_flags(mult).missing_required_inputs);
        assert!(execution_graph.node_flags(print).missing_required_inputs);

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
        let mut func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        // mult[0] unbound + optional (not wired to anything).
        bind(&mut graph, "mult", 0, Binding::None);
        func_lib.by_name_mut("mult").unwrap().inputs[0].required = false;

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert!(!execution_graph.node_flags(mult).missing_required_inputs);
        assert!(!execution_graph.node_flags(print).missing_required_inputs);
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
        let mut func_lib = test_func_lib(default_hooks());

        let get_b_id = graph.by_name("get_b").unwrap().id;
        let sum_id = graph.by_name("sum").unwrap().id;

        // sum's required input[0] unbound → sum missing-required → gated.
        bind(&mut graph, "sum", 0, Binding::None);
        // mult[0] (required) gets a real value; mult[1] is the only bind to the
        // gated sum and is *optional* — so this exercises optional-bind
        // propagation specifically. mult and print end up gated.
        bind(&mut graph, "mult", 0, (get_b_id, 0).into());
        bind(&mut graph, "mult", 1, (sum_id, 0).into());
        func_lib.by_name_mut("mult").unwrap().inputs[1].required = false;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
        // Pre-fix, this panicked the worker; now the chain is gated and nothing runs.
        execution_graph.execute_terminals().await?;

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(execution_graph.node_flags(mult).missing_required_inputs);
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
        let func_lib = test_func_lib(TestFuncHooks::default());

        let sum_id = graph.by_name("sum").unwrap().id;
        graph.by_id_mut(&sum_id).unwrap().disabled = true;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
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
        assert!(!execution_graph.node_flags(get_b).missing_required_inputs);
        assert!(execution_graph.node_flags(mult).missing_required_inputs);
        assert!(execution_graph.node_flags(print).missing_required_inputs);

        Ok(())
    }

    /// With `mult`'s sum-fed input made optional, disabling `sum` no longer
    /// breaks the chain: `sum` is skipped but `get_b → mult → print` still
    /// runs (mirrors `non_required_missing_does_not_propagate`, but via the
    /// disable flag rather than a cleared binding).
    #[test]
    fn disabled_upstream_with_optional_consumer_still_runs() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());

        let sum_id = graph.by_name("sum").unwrap().id;
        graph.by_id_mut(&sum_id).unwrap().disabled = true;
        func_lib.by_name_mut("mult").unwrap().inputs[0].required = false;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(default_hooks());
        let mut execution_graph = ExecutionEngine::default();

        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(3)));
        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(5)));

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        // Only mult and print execute — the const binds detach mult from its
        // upstream, so get_a/get_b/sum are pruned.
        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Re-run with the same bindings: mult's digest is unchanged, so it's a
        // RAM cache hit; only print (impure terminal) re-executes.
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;
        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        // Change one const: mult's digest changes ⇒ cache miss ⇒ it re-executes.
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(4)));
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();
        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );
        assert!(execution_graph.node_flags(mult).wants_execute);
        assert!(!execution_graph.node_flags(mult).cached);
        assert!(!execution_graph.node_flags(mult).missing_required_inputs);
        assert!(!execution_graph.node_flags(print).missing_required_inputs);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn const_binding_invokes_only_once() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || unreachable!()),
            get_b: Arc::new(move || unreachable!()),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(3)));
        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(5)));

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Same const value: no re-execution of mult
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(3)));
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        // Different const value: mult re-executes
        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(4)));
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Stable again
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn const_excludes_upstream_node() -> anyhow::Result<()> {
        let func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        // Replace sum[0] (get_a) with a const — get_a is no longer needed
        bind(&mut graph, "sum", 0, Binding::Const(33.into()));

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "sum", "mult", "print"]
        );

        // Also unbind sum[1] — now sum has all const/none inputs, no upstream needed
        bind(&mut graph, "sum", 1, Binding::None);

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["sum", "mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn change_from_const_to_bind_recomputes() -> anyhow::Result<()> {
        let func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        let get_b_id = graph.by_name_mut("get_b").unwrap().id;
        bind(&mut graph, "sum", 0, Binding::Const(33.into()));

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "sum", "mult", "print"]
        );

        // Switch from const back to bind — sum must re-execute
        bind(&mut graph, "sum", 0, (get_b_id, 0).into());

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["sum", "mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn optional_input_binding_change_recomputes() -> anyhow::Result<()> {
        let func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        // Switch mult inputs to const/none
        bind(&mut graph, "mult", 0, Binding::Const(2.into()));
        bind(&mut graph, "mult", 1, Binding::None);

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Stable on rerun
        execution_graph.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        assert!(execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

        // Simulate cached output — pure node should skip
        execution_graph.set_output_values("get_b", vec![DynamicValue::Static(StaticValue::Int(7))]);

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        assert!(!execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn default_node_skips_on_rerun() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(default_hooks());
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(default_hooks());
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

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
        let func_lib = test_func_lib(default_hooks());
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

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
        use crate::function::{Func, FuncLib, FuncOutput};
        use crate::graph::{Graph, NodeId};

        // Trips the cancel on its first invoke only, so the re-run completes.
        let cancel_first = Arc::new(AtomicBool::new(true));
        let func_lib: FuncLib = [Func {
            id: "8400cb3a-a5d2-4fcd-a9d8-0ab4880c710f".into(),
            name: "self_cancel".to_string(),
            description: None,
            category: "Debug".to_string(),
            behavior: FuncBehavior::Pure,
            terminal: true,
            inputs: vec![],
            outputs: vec![FuncOutput {
                name: "out".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(
                move |ctx, _, _, _, _, outputs| { cancel_first = Arc::clone(&cancel_first) } => {
                    if cancel_first.swap(false, Ordering::Relaxed) {
                        // Stand in for the user hitting Cancel while this node runs.
                        ctx.cancel_flag().cancel();
                    }
                    outputs[0] = DynamicValue::Static(StaticValue::Int(7));
                    Ok(())
                }
            ),
            ..Default::default()
        }]
        .into();

        let mut graph = Graph::default();
        let node_id: NodeId = "acb11422-9951-4fc6-9696-53b1a6699120".into();
        let mut node: Node = func_lib.by_name("self_cancel").unwrap().into();
        node.id = node_id;
        graph.add(node);
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

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
                [NodeError { node_id: n, error: Error::Cancelled { .. } }] if *n == node_id
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
    /// `Error::Cancelled` (not a generic `Invoke` error) and dropped from the
    /// executed set — the truthful lambda-level signal, distinct from the
    /// executor's flag-check fallback covered above (asserted here without
    /// touching the flag, so only the error mapping can produce the verdict).
    #[tokio::test(flavor = "multi_thread")]
    async fn lambda_cancelled_error_maps_to_error_cancelled() -> anyhow::Result<()> {
        use crate::async_lambda;
        use crate::execution_stats::NodeError;
        use crate::func_lambda::InvokeError;
        use crate::function::{Func, FuncLib, FuncOutput};
        use crate::graph::{Graph, NodeId};

        let func_lib: FuncLib = [Func {
            id: "8003e30b-0417-474d-a77f-1d3ea71ac6b3".into(),
            name: "always_cancel".to_string(),
            description: None,
            category: "Debug".to_string(),
            behavior: FuncBehavior::Pure,
            terminal: true,
            inputs: vec![],
            outputs: vec![FuncOutput {
                name: "out".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![],
            required_contexts: vec![],
            lambda: async_lambda!(move |_, _, _, _, _, _| { Err(InvokeError::Cancelled) }),
            ..Default::default()
        }]
        .into();

        let mut graph = Graph::default();
        let node_id: NodeId = "c791f8aa-3bf9-435d-8530-f3904b4b6a28".into();
        let mut node: Node = func_lib.by_name("always_cancel").unwrap().into();
        node.id = node_id;
        graph.add(node);
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();
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
                [NodeError { node_id: n, error: Error::Cancelled { .. } }] if *n == node_id
            ),
            "InvokeError::Cancelled maps to Error::Cancelled, not Invoke: {:?}",
            stats.node_errors
        );

        Ok(())
    }

    #[test]
    fn impure_node_always_invoked() -> anyhow::Result<()> {
        let graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());

        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();

        // Even with cached output, impure node still wants to execute
        execution_graph.set_output_values("get_b", vec![DynamicValue::Static(StaticValue::Int(7))]);
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.prepare_execution(true, false, &[])?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "print"]
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

    fn func_node(func_lib: &FuncLib, func_name: &str, node_name: &str) -> Node {
        let id = func_lib.by_name(func_name).unwrap().id;
        let mut n = Node::new(NodeKind::Func(id));
        n.name = node_name.to_string();
        n
    }

    fn int_output(name: &str) -> FuncOutput {
        FuncOutput {
            name: name.to_string(),
            data_type: DataType::Int,
        }
    }

    /// A subgraph def with no inputs and one output, whose interior is the
    /// impure `get_b` (named `inner_name`) feeding `SubgraphOutput[0]`.
    fn impure_output_def(
        func_lib: &FuncLib,
        id: &str,
        name: &str,
        inner_name: &str,
    ) -> SubgraphDef {
        let inner = func_node(func_lib, "get_b", inner_name);
        let inner_id = inner.id;
        let so = Node::new(NodeKind::SubgraphOutput);
        let so_id = so.id;
        let mut interior = Graph::default();
        interior.add(inner);
        interior.add(so);
        interior.set_input_binding(InputPort::new(so_id, 0), (inner_id, 0).into());
        SubgraphDef {
            id: id.into(),
            name: name.to_string(),
            category: String::new(),
            graph: interior,
            inputs: vec![],
            outputs: vec![int_output("Out")],
            events: vec![],
            origin: None,
        }
    }

    /// Main graph: one instance of `def` whose output feeds a terminal `print`.
    fn main_with(func_lib: &FuncLib, def: SubgraphDef) -> Graph {
        let def_id = def.id;
        let mut graph = Graph::default();
        graph.subgraphs.add(def.clone());
        let inst = graph.add_subgraph_node(&def, SubgraphRef::Local(def_id));
        let p = func_node(func_lib, "print", "p");
        let p_id = p.id;
        graph.add(p);
        graph.set_input_binding(InputPort::new(p_id, 0), (inst, 0).into());
        graph
    }

    /// `(name in execute_order)` after a second prepare, with a cached
    /// output already present for that node — i.e. "would it re-run?".
    fn reruns_with_cache(graph: &Graph, func_lib: &FuncLib, name: &str) -> bool {
        let mut eg = ExecutionEngine::default();
        eg.update(graph, func_lib).unwrap();
        eg.prepare_execution(true, false, &[]).unwrap();
        assert!(
            execution_node_names_in_order(&eg).contains(&name.to_string()),
            "{name} should run on the first prepare"
        );
        eg.set_output_values(name, vec![DynamicValue::Static(StaticValue::Int(11))]);
        eg.update(graph, func_lib).unwrap();
        eg.prepare_execution(true, false, &[]).unwrap();
        execution_node_names_in_order(&eg).contains(&name.to_string())
    }

    #[test]
    fn composite_reruns_impure_interior() {
        // An impure interior recomputes across a composite boundary like any
        // impure node — flattening must preserve its impurity.
        let mut func_lib = test_func_lib(TestFuncHooks::default());
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;
        let def = impure_output_def(
            &func_lib,
            "00000000-0000-0000-0000-0000000000a1",
            "S",
            "inner",
        );
        let graph = main_with(&func_lib, def);
        assert!(
            reruns_with_cache(&graph, &func_lib, "inner"),
            "impure interior recomputes through a composite"
        );
    }

    #[test]
    fn update_rejects_func_missing_inside_subgraph() {
        // The check descends composites: a func only the *interior*
        // references, absent from the lib, is still caught.
        let func_lib = test_func_lib(TestFuncHooks::default());
        let def = impure_output_def(
            &func_lib,
            "00000000-0000-0000-0000-0000000000a1",
            "S",
            "inner",
        );
        let graph = main_with(&func_lib, def);

        // A `Local` def resolves from the graph itself, so the walk reaches
        // the interior even with an empty library — and flags its `get_b`.
        let mut eg = ExecutionEngine::default();
        let err = eg.update(&graph, &FuncLib::default()).unwrap_err();
        let Error::InvalidGraph { message } = err else {
            panic!("expected InvalidGraph, got {err:?}");
        };
        let get_b = func_lib.by_name("get_b").unwrap().id;
        assert!(
            message.contains(&format!("{get_b:?}")),
            "message should name the interior's missing func, got: {message}"
        );
    }

    #[test]
    fn nested_impure_interior_reruns() {
        // A doubly-nested impure node recomputes — flattening preserves its
        // impurity through two composite levels.
        let mut func_lib = test_func_lib(TestFuncHooks::default());
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;
        let inner_def = impure_output_def(
            &func_lib,
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
        let outer_def = SubgraphDef {
            id: "00000000-0000-0000-0000-0000000000b2".into(),
            name: "Outer".to_string(),
            category: String::new(),
            graph: outer_interior,
            inputs: vec![],
            outputs: vec![int_output("Out")],
            events: vec![],
            origin: None,
        };
        let graph = main_with(&func_lib, outer_def);
        assert!(
            reruns_with_cache(&graph, &func_lib, "deep"),
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
        let func_lib = test_func_lib(TestFuncHooks::default());

        // Create cycle: sum[0] ← mult (mult already depends on sum)
        let mult_node_id = graph.by_name("mult").unwrap().id;
        bind(&mut graph, "sum", 0, (mult_node_id, 0).into());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();

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
        let func_lib = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        // Verify outputs exist before reset
        let sum = execution_graph.by_name("sum").unwrap();
        assert!(execution_graph.runtime_slot(sum).output_values.is_some());

        execution_graph.reset_states();

        // All output_values and state should be cleared
        for (e_node, slot) in execution_graph
            .program
            .e_nodes
            .iter()
            .zip(execution_graph.runtime_slots())
        {
            assert!(
                slot.output_values.is_none(),
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
        let mut func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(test_values_a.try_lock().unwrap().a)),
            get_b: Arc::new(move || test_values_b.try_lock().unwrap().b),
            print: Arc::new(move |result| {
                test_values_result.try_lock().unwrap().result = result;
            }),
        });

        let graph = test_graph();

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;
        // sum = get_a + get_b = 2 + 5 = 7, mult = sum * get_b = 7 * 5 = 35
        assert_eq!(test_values.try_lock()?.result, 35);

        // Changing external state doesn't recompute: get_b is pure, so its digest
        // is stable and the cached value stands.
        test_values.try_lock()?.b = 7;

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;
        assert_eq!(test_values.try_lock()?.result, 35);

        // Make get_b Impure: now it re-reads the value
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;
        // sum = 2 + 7 = 9, mult = 9 * 7 = 63
        assert_eq!(test_values.try_lock()?.result, 63);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn required_none_binding_is_stable() -> anyhow::Result<()> {
        let func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        // Make sum's first input None (required) — sum and downstream shouldn't execute
        bind(&mut graph, "sum", 0, Binding::None);

        execution_graph.update(&graph, &func_lib).unwrap();

        execution_graph.execute_terminals().await?;
        let order1 = execution_node_names_in_order(&execution_graph);

        execution_graph.execute_terminals().await?;
        let order2 = execution_node_names_in_order(&execution_graph);

        // Execution order should be stable across runs
        assert_eq!(order1, order2);

        // sum should be marked as missing required inputs
        let sum = execution_graph.by_name("sum").unwrap();
        assert!(execution_graph.node_flags(sum).missing_required_inputs);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn schedule_stable_across_repeated_runs() -> anyhow::Result<()> {
        let func_lib = test_func_lib(default_hooks());
        let graph = test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

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
        let func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        // Switch mult to const inputs
        bind(&mut graph, "mult", 0, Binding::Const(2.into()));
        bind(&mut graph, "mult", 1, Binding::Const(21.into()));

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        // Switch back to bind from cached get_b — mult re-executes with cached upstream
        let get_b_id = graph.by_name_mut("get_b").unwrap().id;
        bind(&mut graph, "mult", 0, (get_b_id, 0).into());

        execution_graph.update(&graph, &func_lib).unwrap();
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
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
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &func_lib).unwrap();

        let nonexistent_id: NodeId = "00000000-0000-0000-0000-000000000000".into();
        assert!(
            execution_graph
                .get_argument_values(&nonexistent_id)
                .is_none()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn with_const_bindings() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || unreachable!()),
            get_b: Arc::new(move || unreachable!()),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        bind(&mut graph, "mult", 0, Binding::Const(StaticValue::Int(3)));
        bind(&mut graph, "mult", 1, Binding::Const(StaticValue::Int(5)));
        let mult_id = graph.by_name("mult").unwrap().id;

        execution_graph.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(2)),
            get_b: Arc::new(move || 5),
            print: Arc::new(move |_| {}),
        });

        let graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &func_lib).unwrap();
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
        let mut func_lib = test_func_lib(default_hooks());

        let mut graph = test_graph();
        let mut execution_graph = ExecutionEngine::default();

        func_lib.by_name_mut("mult").unwrap().inputs[1].required = false;
        bind(&mut graph, "mult", 1, Binding::None);
        let mult_id = graph.by_name("mult").unwrap().id;

        execution_graph.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionEngine::default();

        execution_graph.update(&graph, &func_lib).unwrap();

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
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Err(anyhow::anyhow!("Intentional failure in get_a"))),
            get_b: Arc::new(|| 42),
            print: Arc::new(|_| {}),
        });

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();

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
                .output_values
                .clone()
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
                    .contains("upstream error"),
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
        let func_lib = test_func_lib(default_hooks());

        // Remove sum's first input (required)
        bind(&mut graph, "sum", 0, Binding::None);

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(default_hooks());

        let mut execution_graph = ExecutionEngine::default();
        execution_graph.update(&graph, &func_lib).unwrap();
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
    use crate::function::{Func, FuncEvent, FuncInput, FuncOutput};

    const EMIT_FUNC: FuncId = FuncId::from_u128(0xE311);
    const RECV_FUNC: FuncId = FuncId::from_u128(0xE322);

    struct EventFixture {
        func_lib: FuncLib,
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

        // Fields left unset (behavior, terminal, etc.) match Func::default():
        // both funcs are Impure non-terminals.
        let mut func_lib = FuncLib::default();
        func_lib.add(Func {
            id: EMIT_FUNC,
            name: "emit".to_string(),
            outputs: vec![FuncOutput {
                name: "out".to_string(),
                data_type: DataType::Int,
            }],
            events: vec![FuncEvent {
                name: "tick".to_string(),
                event_lambda: EventLambda::new(|_state| Box::pin(async move {})),
            }],
            lambda: async_lambda!(
                move |_, _, _, _, _, outputs| { calls = emit_calls_l.clone() } => {
                    let mut n = calls.lock().await;
                    *n += 1;
                    outputs[0] = DynamicValue::Static(StaticValue::Int(*n));
                    Ok(())
                }
            ),
            ..Default::default()
        });
        func_lib.add(Func {
            id: RECV_FUNC,
            name: "recv".to_string(),
            inputs: vec![FuncInput {
                name: "in".to_string(),
                required: true,
                data_type: DataType::Int,
                const_only: false,
                default_value: None,
                value_variants: vec![],
            }],
            lambda: async_lambda!(
                move |_, _, _, inputs, _, _| { values = recv_values_l.clone() } => {
                    values.lock().await.push(inputs[0].value.as_i64().unwrap());
                    Ok(())
                }
            ),
            ..Default::default()
        });

        let emit_id = NodeId::unique();
        let recv_id = NodeId::unique();

        let mut graph = Graph::default();
        graph.add(node(&func_lib, "emit", emit_id));
        graph.add(node(&func_lib, "recv", recv_id));
        graph.subscribe(emit_id, 0, recv_id);
        graph.set_input_binding(InputPort::new(recv_id, 0), (emit_id, 0).into());
        graph.validate();

        EventFixture {
            func_lib,
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
        eg.update(&f.graph, &f.func_lib).unwrap();

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
        eg.update(&f.graph, &f.func_lib).unwrap();

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
        eg.update(&f.graph, &f.func_lib).unwrap();

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
        f.func_lib.by_name_mut("emit").unwrap().terminal = true;

        let mut eg = ExecutionEngine::default();
        eg.update(&f.graph, &f.func_lib).unwrap();
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
        function::{Func, FuncInput, FuncOutput},
    };

    const SPLIT_FUNC: FuncId = FuncId::from_u128(0x5911);
    const SINK_FUNC: FuncId = FuncId::from_u128(0x5922);

    #[tokio::test(flavor = "multi_thread")]
    async fn unused_output_marked_skip() -> anyhow::Result<()> {
        let seen_usage: Arc<Mutex<Vec<OutputUsage>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_usage_l = seen_usage.clone();

        let mut func_lib = FuncLib::default();
        func_lib.add(Func {
            id: SPLIT_FUNC,
            name: "split".to_string(),
            outputs: vec![
                FuncOutput {
                    name: "a".to_string(),
                    data_type: DataType::Int,
                },
                FuncOutput {
                    name: "b".to_string(),
                    data_type: DataType::Int,
                },
            ],
            lambda: async_lambda!(
                move |_, _, _, _, usage, outputs| { seen = seen_usage_l.clone() } => {
                    seen.lock().await.extend_from_slice(usage);
                    outputs[0] = DynamicValue::Static(StaticValue::Int(1));
                    outputs[1] = DynamicValue::Static(StaticValue::Int(2));
                    Ok(())
                }
            ),
            ..Default::default()
        });
        func_lib.add(Func {
            id: SINK_FUNC,
            name: "sink".to_string(),
            terminal: true,
            inputs: vec![FuncInput {
                name: "in".to_string(),
                required: true,
                data_type: DataType::Int,
                const_only: false,
                default_value: None,
                value_variants: vec![],
            }],
            lambda: async_lambda!(|_, _, _, _, _, _| { Ok(()) }),
            ..Default::default()
        });

        let split_id = NodeId::unique();
        let sink_id = NodeId::unique();
        let mut graph = Graph::default();
        graph.add(node(&func_lib, "split", split_id));
        graph.add(node(&func_lib, "sink", sink_id));
        // Consume only output 0; output 1 has no consumer.
        graph.set_input_binding(InputPort::new(sink_id, 0), (split_id, 0).into());
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 5),
            print: Arc::new(move |v| *printed_l.try_lock().unwrap() = v),
        });

        let mut graph = test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();
        assert_eq!(eg.program.e_nodes.len(), 5);

        // Remove get_b — a middle node feeding sum[1] and mult[1] (both optional).
        // Forces compaction and target_idx remapping for the survivors.
        let get_b_id = graph.by_name("get_b").unwrap().id;
        graph.remove_by_id(get_b_id);
        graph.validate();

        eg.update(&graph, &func_lib).unwrap();
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
        let func_lib = FuncLib::default();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

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
        let func_lib = test_func_lib(TestFuncHooks {
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
        graph.add(node(&func_lib, "get_a", get_a_id));
        graph.add(node(&func_lib, "get_b", get_b_id));
        graph.add(node(&func_lib, "print", print1_id));
        graph.add(node(&func_lib, "print", print2_id));
        graph.set_input_binding(InputPort::new(print1_id, 0), (get_a_id, 0).into());
        graph.set_input_binding(InputPort::new(print2_id, 0), (get_b_id, 0).into());
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(TestFuncHooks {
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
        graph.add(node(&func_lib, "get_b", get_b_id));
        graph.add(node(&func_lib, "print", print_b_id));
        graph.add(node(&func_lib, "get_a", get_a_id));
        graph.add(node(&func_lib, "print", print_a_id));
        graph.set_input_binding(InputPort::new(print_b_id, 0), (get_b_id, 0).into());
        graph.set_input_binding(InputPort::new(print_a_id, 0), (get_a_id, 0).into());
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();
        eg.execute_terminals().await?;
        assert_eq!(*calls_a.lock().await, 1); // get_a ran once
        let idx_before = eg.program.e_nodes.index_of_key(&get_a_id).unwrap();

        // Remove get_b's chain — get_a's slot compacts toward the front.
        graph.remove_by_id(get_b_id);
        graph.remove_by_id(print_b_id);
        graph.validate();

        eg.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 5),
            print: Arc::new(move |v| p.try_lock().unwrap().push(v)),
        });

        let get_a_id = NodeId::unique();
        let print_a_id = NodeId::unique();
        let mut graph = Graph::default();
        graph.add(node(&func_lib, "get_a", get_a_id));
        graph.add(node(&func_lib, "print", print_a_id));
        graph.set_input_binding(InputPort::new(print_a_id, 0), (get_a_id, 0).into());
        graph.validate();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();
        eg.execute_terminals().await?;

        for round in 0..3 {
            // Add a get_b → print chain.
            let gb = NodeId::unique();
            let pb = NodeId::unique();
            graph.add(node(&func_lib, "get_b", gb));
            graph.add(node(&func_lib, "print", pb));
            graph.set_input_binding(InputPort::new(pb, 0), (gb, 0).into());
            graph.validate();
            eg.update(&graph, &func_lib).unwrap();
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
            eg.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Ok(2)),
            get_b: Arc::new(|| 5),
            print: Arc::new(|_| {}),
        });
        let graph = test_graph();

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();
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
    use crate::function::{Func, FuncBehavior, FuncEvent, FuncId, FuncOutput};
    use crate::graph::NodeKind;
    use crate::prelude::FuncLambda;
    use crate::subgraph::{SubgraphDef, SubgraphEvent, SubgraphId, SubgraphRef};
    use std::sync::Mutex as StdMutex;

    fn fnode(func_lib: &FuncLib, name: &str) -> Node {
        func_lib.by_name(name).unwrap().into()
    }

    fn int_out(name: &str) -> FuncOutput {
        FuncOutput {
            name: name.into(),
            data_type: DataType::Int,
        }
    }

    /// `in(A,B) -> sum -> out(Sum)`.
    fn wrap_sum_def(func_lib: &FuncLib) -> SubgraphDef {
        let in_node = Node::new(NodeKind::SubgraphInput);
        let in_id = in_node.id;
        let sum = fnode(func_lib, "sum");
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

        SubgraphDef {
            id: SubgraphId::unique(),
            name: "WrapSum".into(),
            category: "Test".into(),
            graph,
            inputs: vec![
                crate::function::FuncInput {
                    name: "A".into(),
                    required: true,
                    data_type: DataType::Int,
                    const_only: false,
                    default_value: None,
                    value_variants: vec![],
                },
                crate::function::FuncInput {
                    name: "B".into(),
                    required: false,
                    data_type: DataType::Int,
                    const_only: false,
                    default_value: None,
                    value_variants: vec![],
                },
            ],
            outputs: vec![int_out("Sum")],
            events: vec![],
            origin: None,
        }
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
        let func_lib = test_func_lib(hooks);
        let def = wrap_sum_def(&func_lib);

        let get_a = fnode(&func_lib, "get_a");
        let get_b = fnode(&func_lib, "get_b");
        let (a_id, b_id) = (get_a.id, get_b.id);
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;
        let print = fnode(&func_lib, "print");
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
        eg.update(&graph, &func_lib).unwrap();
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
        let func_lib = test_func_lib(hooks);

        // def TwoSources: get_a -> out0, get_b -> out1 (no inputs).
        let src_a = fnode(&func_lib, "get_a");
        let src_b = fnode(&func_lib, "get_b");
        let (sa, sb) = (src_a.id, src_b.id);
        let out = Node::new(NodeKind::SubgraphOutput);
        let out_id = out.id;
        let mut def_graph = Graph::default();
        def_graph.add(src_a);
        def_graph.add(src_b);
        def_graph.add(out);
        def_graph.set_input_binding(InputPort::new(out_id, 0), (sa, 0).into());
        def_graph.set_input_binding(InputPort::new(out_id, 1), (sb, 0).into());
        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "TwoSources".into(),
            category: "Test".into(),
            graph: def_graph,
            inputs: vec![],
            outputs: vec![int_out("O0"), int_out("O1")],
            events: vec![],
            origin: None,
        };

        // parent: C, print <- C.out0 (out1 unused).
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;
        let print = fnode(&func_lib, "print");
        let print_id = print.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(c);
        graph.add(print);
        graph.set_input_binding(InputPort::new(print_id, 0), (c_id, 0).into());

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();
        eg.execute_terminals().await?;

        assert_eq!(*captured.lock().unwrap(), vec![7]); // get_a only; get_b pruned
        Ok(())
    }

    /// A data cycle that runs through a composite boundary is caught by the
    /// existing cycle detector once flattened.
    #[tokio::test(flavor = "multi_thread")]
    async fn cross_boundary_cycle_detected() {
        let func_lib = test_func_lib(default_hooks());
        let def = wrap_sum_def(&func_lib);

        // C.in0 <- C.out0 (self-cycle through the composite); print <- C.out0
        // so the cyclic node is reachable from a terminal.
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;
        let print = fnode(&func_lib, "print");
        let print_id = print.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(c);
        graph.add(print);
        graph.set_input_binding(InputPort::new(c_id, 0), (c_id, 0).into());
        graph.set_input_binding(InputPort::new(print_id, 0), (c_id, 0).into());

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();
        let result = eg.execute_terminals().await;

        assert!(
            matches!(result, Err(Error::CycleDetected { .. })),
            "expected CycleDetected, got {result:?}"
        );
    }

    fn bind_target(eg: &ExecutionEngine, e: &ExecutionNode, input_idx: usize) -> NodeId {
        match &eg.node_inputs(e)[input_idx].binding {
            ExecutionBinding::Bind(addr) => addr.target_id,
            other => panic!("expected Bind, got {other:?}"),
        }
    }

    /// A composite dissolves: only its interior func leaves remain, wired
    /// directly to the parent's producers/consumers.
    #[test]
    fn composite_dissolves_into_leaf_edges() {
        // get_a, get_b -> C(WrapSum) -> print.
        let func_lib = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum_def(&func_lib);

        let get_a = fnode(&func_lib, "get_a");
        let get_b = fnode(&func_lib, "get_b");
        let (a_id, b_id) = (get_a.id, get_b.id);
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;
        let print = fnode(&func_lib, "print");
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
        eg.update(&graph, &func_lib).unwrap();

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
        let func_lib = test_func_lib(default_hooks());
        let graph = test_graph();
        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

        assert_eq!(eg.program.e_nodes.len(), graph.len());
        for node in graph.iter() {
            assert!(eg.by_id(&node.id).is_some(), "id preserved");
        }
    }

    /// Two instances of one def produce two distinct interior leaves.
    #[test]
    fn two_instances_get_distinct_leaf_ids() {
        let mut func_lib = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum_def(&func_lib);
        let def_id = def.id;
        func_lib.add_subgraph(def);

        let mut graph = Graph::default();
        let def_ref = func_lib.subgraph_by_id(&def_id).unwrap();
        graph.add(Node::subgraph_instance(
            def_ref,
            SubgraphRef::Linked(def_id),
        ));
        graph.add(Node::subgraph_instance(
            def_ref,
            SubgraphRef::Linked(def_id),
        ));

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

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
        let func_lib = test_func_lib(TestFuncHooks::default());
        let def = wrap_sum_def(&func_lib);
        // The id the editor knows the interior node by (in the def graph).
        let interior_sum_id = def.graph.iter().find(|n| n.name == "sum").unwrap().id;

        let get_a = fnode(&func_lib, "get_a");
        let a_id = get_a.id;
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(get_a);
        graph.add(c);
        graph.set_input_binding(InputPort::new(c_id, 0), (a_id, 0).into());

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

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
    fn add_ticker(func_lib: &mut FuncLib) {
        func_lib.add(Func {
            id: FuncId::unique(),
            name: "ticker".into(),
            category: "Test".into(),
            terminal: true,
            uncacheable: false,
            behavior: FuncBehavior::Impure,
            version: 0,
            description: None,
            inputs: vec![],
            outputs: vec![],
            events: vec![FuncEvent {
                name: "tick".into(),
                event_lambda: EventLambda::default(),
            }],
            required_contexts: vec![],
            lambda: FuncLambda::default(),
        });
    }

    fn func_lib_with_ticker() -> FuncLib {
        let mut func_lib = test_func_lib(default_hooks());
        add_ticker(&mut func_lib);
        func_lib
    }

    fn subscriber_ids(eg: &ExecutionEngine, e: &ExecutionNode, event_idx: usize) -> Vec<NodeId> {
        eg.node_events(e)[event_idx].subscribers.clone()
    }

    /// A parent subscriber of a composite's exposed event is rewired onto the
    /// flattened interior emitter.
    #[test]
    fn exposed_event_rewires_parent_subscriber_to_interior_emitter() {
        let func_lib = func_lib_with_ticker();

        // def: a single `ticker`, its `tick` event exposed as the composite's
        // event 0.
        let emitter = fnode(&func_lib, "ticker");
        let emitter_id = emitter.id;
        let mut def_graph = Graph::default();
        def_graph.add(emitter);
        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "Exposer".into(),
            category: "Test".into(),
            graph: def_graph,
            inputs: vec![],
            outputs: vec![],
            events: vec![SubgraphEvent {
                name: "tick".into(),
                emitter: emitter_id,
                emitter_event_idx: 0,
            }],
            origin: None,
        };

        // parent: composite C, and `listener` subscribing to C's event 0.
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;
        let listener = fnode(&func_lib, "print");
        let listener_id = listener.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(c);
        graph.add(listener);
        graph.subscribe(c_id, 0, listener_id);

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

        // The flattened interior `ticker` carries the rewired subscriber.
        let ticker_node = eg.by_name("ticker").unwrap();
        assert_eq!(subscriber_ids(&eg, ticker_node, 0), vec![listener_id]);
    }

    /// Triggering a composite (as a subscriber) reaches the interior nodes
    /// wired to its `SubgraphInput` trigger.
    #[test]
    fn triggering_composite_reaches_interior_subscribers() {
        let func_lib = func_lib_with_ticker();

        // def: SubgraphInput trigger → interior `print` subscribes to it.
        let si = Node::new(NodeKind::SubgraphInput);
        let si_id = si.id;
        let reactor = fnode(&func_lib, "print");
        let reactor_id = reactor.id;
        let mut def_graph = Graph::default();
        def_graph.add(si);
        def_graph.add(reactor);
        def_graph.subscribe(si_id, 0, reactor_id);
        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "Reactor".into(),
            category: "Test".into(),
            graph: def_graph,
            inputs: vec![],
            outputs: vec![],
            events: vec![],
            origin: None,
        };

        // parent: `ticker` emits; composite C subscribes to it.
        let emitter = fnode(&func_lib, "ticker");
        let emitter_id = emitter.id;
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(emitter);
        graph.add(c);
        graph.subscribe(emitter_id, 0, c_id);

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

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
        let mut func_lib = test_func_lib(hooks);
        let def = wrap_sum_def(&func_lib);
        let def_id = def.id;
        func_lib.add_subgraph(def);

        // Two linked instances with const inputs, each feeding a print.
        let def_ref = func_lib.subgraph_by_id(&def_id).unwrap();
        let c1 = Node::subgraph_instance(def_ref, SubgraphRef::Linked(def_id));
        let c2 = Node::subgraph_instance(def_ref, SubgraphRef::Linked(def_id));
        let (c1_id, c2_id) = (c1.id, c2.id);
        let p1 = fnode(&func_lib, "print");
        let p2 = fnode(&func_lib, "print");
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
        eg.update(&graph, &func_lib).unwrap();
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
            let def = func_lib.subgraphs.by_key_mut(&def_id).unwrap();
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

        eg.update(&graph, &func_lib).unwrap();
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
        let mut func_lib = test_func_lib(hooks);
        add_ticker(&mut func_lib);

        // def Reactor: SubgraphInput trigger → interior `get_a` subscribes.
        let si = Node::new(NodeKind::SubgraphInput);
        let si_id = si.id;
        let reactor = fnode(&func_lib, "get_a");
        let reactor_id = reactor.id;
        let mut def_graph = Graph::default();
        def_graph.add(si);
        def_graph.add(reactor);
        def_graph.subscribe(si_id, 0, reactor_id);
        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: "Reactor".into(),
            category: "Test".into(),
            graph: def_graph,
            inputs: vec![],
            outputs: vec![],
            events: vec![],
            origin: None,
        };

        // parent: `ticker` E; composite C subscribes to E's event.
        let emitter = fnode(&func_lib, "ticker");
        let emitter_id = emitter.id;
        let c = Node::subgraph_instance(&def, SubgraphRef::Local(def.id));
        let c_id = c.id;

        let mut graph = Graph::default();
        graph.subgraphs.add(def);
        graph.add(emitter);
        graph.add(c);
        graph.subscribe(emitter_id, 0, c_id);

        let mut eg = ExecutionEngine::default();
        eg.update(&graph, &func_lib).unwrap();

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
