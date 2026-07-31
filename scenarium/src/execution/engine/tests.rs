use std::sync::Arc;

use super::*;
use crate::execution::compile::error::CompileError;
use crate::execution::error::{Error, RunError};
use crate::execution::seeds::RunSeeds;
use crate::graph::func::FuncBehavior;
use crate::graph::func::error::InvokeError;
use crate::graph::func::lambda::{Invocation, OutputDemand, internals};
use crate::graph::identity::{NodeId, OutputPort};
use crate::graph::node::CacheMode;
use crate::library::Library;
use crate::testing::TestFuncHooks;
use crate::testing::engine::{RunOutcome, TestEngine};
use crate::testing::graph::{NodeSpec, TestGraph};
use crate::{DataType, DynamicValue, StaticValue};
use ::common::{CancelToken, FloatExt};
use tokio::sync::Mutex;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

mod cache_persistence {
    use super::*;
    use crate::execution::schedule::NodeState;
    use std::path::PathBuf;
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

    /// Count of blobs in the store — one per persisted node.
    fn blob_count(dir: &TempDir) -> usize {
        std::fs::read_dir(&dir.0).unwrap().flatten().count()
    }

    /// An engine over `graph` backed by the store at `dir`. Calling it twice
    /// against one dir is a reopen: fresh RAM, same blobs.
    fn disk_engine(dir: &TempDir, graph: TestGraph) -> TestEngine {
        let mut e = TestEngine::over(graph);
        e.attach_disk_store(dir.0.clone());
        e
    }

    /// A pure source emitting `value` and counting how often it was asked —
    /// the recompute counter every test below reads.
    fn source(value: i64, calls: Arc<AtomicUsize>) -> impl FnOnce(NodeSpec) -> NodeSpec {
        move |n: NodeSpec| {
            n.pure().output(DataType::Int).compute(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                value.into()
            })
        }
    }

    /// A pure two-input arithmetic node on `mode`. The second input is
    /// optional, defaulting to the operation's identity, so a fixture may leave
    /// it unbound.
    fn binop(
        mode: CacheMode,
        identity: i64,
        op: fn(i64, i64) -> i64,
    ) -> impl FnOnce(NodeSpec) -> NodeSpec {
        move |n: NodeSpec| {
            n.pure()
                .cache(mode)
                .input(DataType::Int)
                .optional(DataType::Int)
                .output(DataType::Int)
                .compute(move |inputs| {
                    let a = inputs[0].as_i64().unwrap();
                    let b = inputs[1].as_i64().unwrap_or(identity);
                    op(a, b).into()
                })
        }
    }

    fn mult(mode: CacheMode) -> impl FnOnce(NodeSpec) -> NodeSpec {
        binop(mode, 1, |a, b| a * b)
    }

    fn sum(mode: CacheMode) -> impl FnOnce(NodeSpec) -> NodeSpec {
        binop(mode, 0, |a, b| a + b)
    }

    /// `src → mult(mode) → print`, both of mult's inputs fed by the source.
    /// The sink is impure, so `mult` is demanded every run.
    fn source_mult_print(mode: CacheMode, value: i64, calls: Arc<AtomicUsize>) -> TestGraph {
        let mut g = TestGraph::new();
        g.add("src", source(value, calls));
        g.add("mult", mult(mode));
        g.add("print", |n| n.records());
        g.wire("src", 0, "mult", 0);
        g.wire("src", 0, "mult", 1);
        g.wire("mult", 0, "print", 0);
        g
    }

    #[tokio::test]
    async fn explicit_cache_eviction_removes_the_downstream_ram_and_disk_cone() {
        let dir = TempDir::new("explicit-eviction");
        let (a_calls, b_calls) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));

        // src_a → sum → mult → print, with src_b feeding sum's second input —
        // an upstream sibling *outside* src_a's consumer cone.
        let mut g = TestGraph::new();
        g.add("src_a", |n| {
            source(1, a_calls.clone())(n).cache(CacheMode::Both)
        });
        g.add("src_b", |n| {
            source(11, b_calls.clone())(n).cache(CacheMode::Both)
        });
        g.add("sum", sum(CacheMode::Both));
        g.add("mult", mult(CacheMode::Both));
        g.add("print", |n| n.records());
        g.wire("src_a", 0, "sum", 0);
        g.wire("src_b", 0, "sum", 1);
        g.wire("sum", 0, "mult", 0);
        g.wire("src_b", 0, "mult", 1);
        g.wire("mult", 0, "print", 0);

        let mut e = disk_engine(&dir, g);
        e.run_sinks().await;
        let run = e.run_sinks().await;
        assert_eq!(a_calls.load(Ordering::SeqCst), 1);
        assert_eq!(b_calls.load(Ordering::SeqCst), 1);
        assert_eq!(run.logs(), ["132"], "(1 + 11) * 11");
        assert_eq!(blob_count(&dir), 4);

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
        assert_eq!(blob_count(&dir), 1, "only src_b's disk blob remains");

        // Reopening recomputes exactly what was evicted; the retained sibling
        // is still served from its blob.
        drop(e);
        let mut e = disk_engine(&dir, source_cone(&a_calls, &b_calls));
        let run = e.run_sinks().await;
        assert_eq!(a_calls.load(Ordering::SeqCst), 2);
        assert_eq!(b_calls.load(Ordering::SeqCst), 1);
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
        let blocked = dir.0.join(e.id("src_a").as_uuid().simple().to_string());
        std::fs::remove_file(&blocked).unwrap();
        std::fs::create_dir(&blocked).unwrap();

        let failures = e.evict(["src_a"]).await;
        let [failure] = failures.as_slice() else {
            panic!("the undeletable src_a path must be the only eviction failure");
        };
        assert_eq!(failure.node_id, e.id("src_a"));
        assert!(
            failure
                .message
                .starts_with(&format!("failed to remove {}:", blocked.display()))
        );
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

    /// The eviction fixture's graph, rebuilt for a reopened engine. Declaration
    /// order fixes the ids, so the new engine addresses the same slots.
    fn source_cone(a_calls: &Arc<AtomicUsize>, b_calls: &Arc<AtomicUsize>) -> TestGraph {
        let mut g = TestGraph::new();
        g.add("src_a", |n| {
            source(1, a_calls.clone())(n).cache(CacheMode::Both)
        });
        g.add("src_b", |n| {
            source(11, b_calls.clone())(n).cache(CacheMode::Both)
        });
        g.add("sum", sum(CacheMode::Both));
        g.add("mult", mult(CacheMode::Both));
        g.add("print", |n| n.records());
        g.wire("src_a", 0, "sum", 0);
        g.wire("src_b", 0, "sum", 1);
        g.wire("sum", 0, "mult", 0);
        g.wire("src_b", 0, "mult", 1);
        g.wire("mult", 0, "print", 0);
        g
    }

    /// A disk-persisted node's output survives a fresh engine (reopen), its
    /// sole-consumer upstream is pruned on the hit, and an input change
    /// invalidates it — *overwriting* the node's one blob rather than orphaning
    /// it beside a new one.
    #[tokio::test]
    async fn persist_output_survives_reopen_and_invalidates_on_digest_change() {
        let dir = TempDir::new("e2e");
        let calls = Arc::new(AtomicUsize::new(0));
        let build = |calls: &Arc<AtomicUsize>| source_mult_print(CacheMode::Disk, 7, calls.clone());

        // First run: everything computes; `mult` is stored to disk.
        let mut e = disk_engine(&dir, build(&calls));
        e.run_sinks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Reopen: `mult` loads from disk. Its only consumer of `src` is the
        // reused `mult`, which never reads it, so the pre-run cut prunes `src`
        // — a RAM-only source with no cross-session cache is *not* recomputed.
        let mut e = disk_engine(&dir, build(&calls));
        let run = e.run_sinks().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
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
            calls.load(Ordering::SeqCst),
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
            blob_count(&dir),
            1,
            "a digest change overwrites the node's blob, not adds a second"
        );
    }

    /// Fan-out: a producer feeding both a reuse hit *and* a running consumer
    /// must survive the cut — the running consumer still reads it. Proves the
    /// cut is a backward union over consumers, not a forward "all consumers
    /// reused" filter (which would wrongly prune the shared producer and starve
    /// the executing branch).
    #[tokio::test]
    async fn shared_producer_read_by_a_running_consumer_is_not_cut() {
        let dir = TempDir::new("fanout");
        let calls = Arc::new(AtomicUsize::new(0));

        // src → mult(Disk) → print_mult ;  src → print_direct.
        let build = |calls: &Arc<AtomicUsize>| {
            let mut g = TestGraph::new();
            g.add("src", source(7, calls.clone()));
            g.add("mult", mult(CacheMode::Disk));
            g.add("print_mult", |n| n.records());
            g.instance("print_direct", "print_mult");
            g.wire("src", 0, "mult", 0);
            g.wire("src", 0, "mult", 1);
            g.wire("mult", 0, "print_mult", 0);
            g.wire("src", 0, "print_direct", 0);
            g
        };

        let mut e = disk_engine(&dir, build(&calls));
        e.run_sinks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Reopen: mult reuses from disk, so the src→mult edge is cut — but
        // print_direct still reads src, so the union keeps src alive.
        let mut e = disk_engine(&dir, build(&calls));
        let run = e.run_sinks().await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
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
        let calls = Arc::new(AtomicUsize::new(0));

        // src(7) → sum(Both) = 14 → mult(Both) = 98 → print. `Both` (RAM + disk)
        // so the frontier the run reads is kept resident — that retention is
        // what this test asserts; pure `Disk` would drop its RAM copy.
        let build = |calls: &Arc<AtomicUsize>| {
            let mut g = TestGraph::new();
            g.add("src", source(7, calls.clone()));
            g.add("sum", sum(CacheMode::Both));
            g.add("mult", mult(CacheMode::Both));
            g.add("print", |n| n.records());
            g.wire("src", 0, "sum", 0);
            g.wire("src", 0, "sum", 1);
            g.wire("sum", 0, "mult", 0);
            g.wire("src", 0, "mult", 1);
            g.wire("mult", 0, "print", 0);
            g
        };

        let mut e = disk_engine(&dir, build(&calls));
        e.run_sinks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Reopen with fresh RAM. Resolution alone settles `mult` as a reuse —
        // its blob header covers the demand — without decoding the body: the
        // value only enters RAM when the run loop reaches the node, so a run's
        // reusable frontier never accumulates ahead of the first lambda.
        let mut e = disk_engine(&dir, build(&calls));
        e.plan_sinks().await.unwrap();
        assert_eq!(
            e.state("mult"),
            NodeState::Reuse,
            "the frontier blob is verified from its header during resolution"
        );
        assert!(!e.holds_output("mult"), "...and is not decoded there");

        let run = e.run_sinks().await;

        assert_eq!(
            calls.load(Ordering::SeqCst),
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
        e.attach_disk_store(empty.0.clone());
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
            calls.load(Ordering::SeqCst),
            2,
            "recomputing sum also restores its pruned memory-only input"
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
        let calls = Arc::new(AtomicUsize::new(0));
        let build = |calls: &Arc<AtomicUsize>| source_mult_print(CacheMode::Disk, 7, calls.clone());

        let mut e = disk_engine(&dir, build(&calls));
        e.run_sinks().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

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
            calls.load(Ordering::SeqCst),
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
            blob_count(&dir),
            0,
            "the undecodable blob is dropped rather than left to fail every future run"
        );

        // Nothing left to reuse: the whole cone recomputes and republishes.
        let run = e.run_sinks().await;
        assert!(run.errored().is_empty(), "the next run is clean");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(blob_count(&dir), 1);
    }

    /// A `Both` value remains resident even when a later run neither executes
    /// nor reads it.
    #[tokio::test]
    async fn both_value_stays_resident_outside_the_active_frontier() {
        let dir = TempDir::new("both-retained");
        let calls = Arc::new(AtomicUsize::new(0));

        // src(1) → sum(Both) = 2 → mult(Both) = 2 → print.
        let mut g = TestGraph::new();
        // `src` retains in RAM but never persists — the "non-reloadable value"
        // the last assertion is about.
        g.add("src", |n| source(1, calls.clone())(n).cache(CacheMode::Ram));
        g.add("sum", sum(CacheMode::Both));
        g.add("mult", mult(CacheMode::Both));
        g.add("print", |n| n.records());
        g.wire("src", 0, "sum", 0);
        g.wire("src", 0, "sum", 1);
        g.wire("sum", 0, "mult", 0);
        g.wire("src", 0, "mult", 1);
        g.wire("mult", 0, "print", 0);

        let mut e = disk_engine(&dir, g);
        e.run_sinks().await;
        assert!(
            e.holds_output("sum"),
            "sum is resident after the run that computed it"
        );

        let run = e.run_sinks().await;
        assert_eq!(run.ran_node_count, 1, "only print runs the second time");
        assert_eq!(
            e.output_i64("sum", 0),
            Some(2),
            "Both keeps the exact prior-run value resident outside the frontier"
        );
        assert!(
            e.holds_output("mult"),
            "the frontier value the run read is kept resident"
        );
        assert!(
            e.holds_output("src"),
            "a non-reloadable memory-only value is kept, never force-recomputed"
        );

        // An empty replacement store proves the later hit comes from retained
        // RAM, not from disk.
        let empty = TempDir::new("both-retained-empty");
        e.attach_disk_store(empty.0.clone());
        e.edit(|g| g.constant("mult", 1, 3i64));
        let run = e.run_sinks().await;
        assert!(
            run.cached().contains(&"sum"),
            "sum is reused from retained RAM"
        );
        assert!(!run.ran().contains(&"sum"), "sum does not recompute");
        assert!(
            run.ran().contains(&"mult"),
            "the changed downstream recomputes"
        );
        assert!(
            e.holds_output("sum"),
            "the reused Both value remains resident"
        );
    }

    /// One row of the cache-mode matrix. Over a fresh store, build
    /// `src → mult(mode) → print` (an impure sink, so `mult` is needed every
    /// run), run twice on one engine, then reopen with empty RAM. Asserts the
    /// four modes' *distinct* outcomes on the axes they differ on: cross-run
    /// reuse, RAM retention after the run, and disk persistence.
    async fn assert_mode_behavior(mode: CacheMode) {
        let dir = TempDir::new(&format!("mode-{mode:?}"));
        let calls = Arc::new(AtomicUsize::new(0));

        let mut e = disk_engine(&dir, source_mult_print(mode, 1, calls.clone()));
        let run1 = e.run_sinks().await;
        assert!(
            run1.ran().contains(&"mult"),
            "{mode:?}: mult computes on the cold run"
        );

        let run2 = e.run_sinks().await;
        if mode == CacheMode::None {
            assert!(
                run2.ran().contains(&"mult"),
                "None recomputes every run its value is needed"
            );
            assert!(
                !run2.cached().contains(&"mult"),
                "None is never reported cached"
            );
        } else {
            assert!(
                run2.cached().contains(&"mult"),
                "{mode:?} reuses its cached output on run 2"
            );
            assert!(
                !run2.ran().contains(&"mult"),
                "{mode:?} does not recompute on run 2"
            );
        }

        // Slot retention after run 2: RAM-resident iff the mode keeps RAM.
        assert_eq!(
            e.holds_output("mult"),
            mode.caches_in_ram(),
            "{mode:?}: RAM retention must equal caches_in_ram()"
        );
        if mode.caches_in_ram() {
            assert_eq!(
                e.output_i64("mult", 0),
                Some(1),
                "{mode:?} keeps the resident value (1 * 1 = 1)"
            );
        }

        // A blob exists iff the mode persists to disk.
        assert_eq!(
            blob_count(&dir) > 0,
            mode.persists_to_disk(),
            "{mode:?}: a blob exists iff persists_to_disk()"
        );

        // Reopen with empty RAM over the same store: only a disk-backed mode
        // survives.
        let mut e = disk_engine(&dir, source_mult_print(mode, 1, calls.clone()));
        let reopen = e.run_sinks().await;
        if mode.persists_to_disk() {
            assert!(
                reopen.cached().contains(&"mult"),
                "{mode:?} reloads mult from disk on reopen"
            );
            assert!(
                !reopen.ran().contains(&"src"),
                "{mode:?}: the cut prunes src behind the disk hit"
            );
        } else {
            assert!(
                reopen.ran().contains(&"mult"),
                "{mode:?} has no disk blob, so mult recomputes on reopen"
            );
            assert!(
                reopen.ran().contains(&"src"),
                "{mode:?}: src recomputes to feed mult"
            );
        }
    }

    /// The four cache modes produce four distinct reuse / retention /
    /// persistence behaviors — the parameterized proof that the mode actually
    /// drives the engine.
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

    /// `None` is storage-only: it never taints downstream reproducibility.
    /// `A(None) → B(Disk)` — B still has a content digest, so it persists and,
    /// on reopen, is served from disk with A cut (not recomputed), exactly as if
    /// A were an ordinary cached node. Contrast an `Impure` A, which *would*
    /// strip B of its digest and force both to rerun.
    #[tokio::test]
    async fn none_upstream_does_not_disable_downstream_disk_cache() {
        let dir = TempDir::new("none-orthogonal");
        let calls = Arc::new(AtomicUsize::new(0));

        // src(1) → a = sum(None) = 2 → b = mult(Disk) = 4 → print.
        let build = |calls: &Arc<AtomicUsize>| {
            let mut g = TestGraph::new();
            g.add("src", source(1, calls.clone()));
            g.add("a", sum(CacheMode::None));
            g.add("b", mult(CacheMode::Disk));
            g.add("print", |n| n.records());
            g.wire("src", 0, "a", 0);
            g.wire("src", 0, "a", 1);
            g.wire("a", 0, "b", 0);
            g.wire("a", 0, "b", 1);
            g.wire("b", 0, "print", 0);
            g
        };

        let mut e = disk_engine(&dir, build(&calls));
        let cold = e.run_sinks().await;
        assert!(
            cold.ran().contains(&"a") && cold.ran().contains(&"b"),
            "the cold run computes A and B"
        );
        assert!(
            blob_count(&dir) > 0,
            "B(Disk) persists despite its None upstream"
        );

        // Reopen: B is a disk hit, so A(None) — read only by the reused B — is
        // cut, not recomputed. Setting A to None disabled neither B's cache nor
        // A's own reuse-cut.
        let mut e = disk_engine(&dir, build(&calls));
        let reopen = e.run_sinks().await;
        assert!(
            reopen.cached().contains(&"b"),
            "B reloads from disk on reopen"
        );
        assert!(
            !reopen.ran().contains(&"a"),
            "A(None) is cut behind the disk hit, not recomputed"
        );
        assert!(!reopen.ran().contains(&"src"), "src is cut behind A too");
    }

    /// A valid disk blob for a node's *current* digest must be served even when
    /// the slot still holds a RAM value produced under a superseded digest — the
    /// stale resident value must not mask the fresh blob. Disk reuse must load
    /// the current blob before deciding an older resident value is reusable.
    ///
    /// The intervening run uses `Ram` mode so it can't overwrite the node's one
    /// disk blob (a `Disk`-mode run would — the blob is keyed by node id).
    #[tokio::test(flavor = "multi_thread")]
    async fn stale_ram_value_does_not_mask_a_valid_disk_blob() -> TestResult {
        let dir = TempDir::new("flip_back");

        // Const binds detach mult from any upstream, so its digest is a pure
        // function of the two consts.
        let build = |a: i64, b: i64, mode: CacheMode| {
            let mut g = TestGraph::new();
            g.add("mult", mult(mode));
            g.add("print", |n| n.records());
            g.constant("mult", 0, a);
            g.constant("mult", 1, b);
            g.wire("mult", 0, "print", 0);
            g
        };

        // Config A (Disk): mult = 2 * 3 = 6 → the blob (digest D_A) on disk.
        let mut e = disk_engine(&dir, build(2, 3, CacheMode::Disk));
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
        Ok(())
    }

    /// A persisted node is written to disk the moment *it* finishes, not in a
    /// batch at the end of the run — so its blob is already on disk by the time
    /// a downstream node executes. The sink checks the store dir is non-empty
    /// when it runs; that holds only because `mult` was persisted right after it
    /// finished. Batched-at-the-end storing would leave the dir empty here.
    #[tokio::test(flavor = "multi_thread")]
    async fn persist_node_lands_on_disk_before_its_consumer_runs() {
        let dir = TempDir::new("per_node_store");
        let root = dir.0.clone();
        let blob_present = Arc::new(AtomicBool::new(false));

        // mult(const 2, const 3) = 6, Disk → sink. Const binds detach mult from
        // any upstream, so only mult and the sink run.
        let mut g = TestGraph::new();
        g.add("mult", mult(CacheMode::Disk));
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

        let mut e = disk_engine(&dir, g);
        e.run_sinks().await;

        assert!(
            blob_present.load(Ordering::SeqCst),
            "mult's disk blob must exist when its consumer runs (persisted \
             per-node, not batched at the end of the run)"
        );
    }

    /// Disabling RAM retention releases a surviving slot during install rather
    /// than waiting for the end of a later run.
    #[tokio::test]
    async fn disabling_ram_retention_releases_resident_value_on_install() {
        for mode in [CacheMode::None, CacheMode::Disk] {
            let dir = TempDir::new(&format!("ram-downgrade-{mode:?}"));
            let mut g = TestGraph::new();
            g.add("mult", mult(CacheMode::Ram));
            g.add("print", |n| n.records());
            g.constant("mult", 0, 2i64);
            g.constant("mult", 1, 3i64);
            g.wire("mult", 0, "print", 0);

            let mut e = disk_engine(&dir, g);
            e.run_sinks().await;
            assert!(e.holds_output("mult"), "Ram retains the current pure value");

            e.edit(|g| g.cache("mult", mode));
            assert!(
                !e.holds_output("mult"),
                "{mode:?} releases the old RAM value during install"
            );
        }
    }

    /// `store_resident_caches` must not write a value under a digest it wasn't
    /// produced under. After an input change recompiles the program, a node's
    /// resident value is stale w.r.t. its new digest; flushing it stamped with
    /// D_B would overwrite the node's blob with bytes a later run at D_B would
    /// load as a false hit.
    #[tokio::test]
    async fn flush_skips_a_value_stale_for_the_current_digest() {
        let dir = TempDir::new("stale_flush");

        let mut g = TestGraph::new();
        g.add("mult", mult(CacheMode::Disk));
        g.add("print", |n| n.records());
        g.constant("mult", 0, 2i64);
        g.constant("mult", 1, 3i64);
        g.wire("mult", 0, "print", 0);

        // Config A: mult runs and is stored, stamped with its digest D_A.
        let mut e = disk_engine(&dir, g);
        e.run_sinks().await;
        assert_eq!(blob_count(&dir), 1, "config A's blob is stored");
        let blob_path = std::fs::read_dir(&dir.0)
            .unwrap()
            .flatten()
            .next()
            .unwrap()
            .path();
        let blob_a = std::fs::read(&blob_path).unwrap();

        // Config B: mult's inputs change ⇒ its *current* digest is now D_B, but
        // the resident value was produced under D_A. Recompile, do not re-run,
        // then flush — the stale value must not be re-stamped D_B. The blob is
        // keyed by node id, so a bad flush shows as an overwrite.
        e.edit(|g| {
            g.constant("mult", 0, 5i64);
            g.constant("mult", 1, 7i64);
        });
        e.engine.store_resident_caches().await;
        assert_eq!(
            std::fs::read(&blob_path).unwrap(),
            blob_a,
            "a value stale for the current digest is not flushed (blob untouched)"
        );
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
        let calls = Arc::new(AtomicUsize::new(0));
        let build = |calls: &Arc<AtomicUsize>| source_mult_print(CacheMode::Disk, 1, calls.clone());

        // Cold run: mult computes and stores its blob.
        {
            let mut e = disk_engine(&dir, build(&calls));
            let run = e.run_sinks().await;
            assert!(run.ran().contains(&"mult"), "the cold run computes mult");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Corrupt mult's blob *body* — a torn write, or an old
        // version-mismatched format — while keeping the leading 32-byte digest
        // header intact: a garbled header would already fail the presence probe
        // and never reach the body verification this test is about.
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
        assert_eq!(calls.load(Ordering::SeqCst), 2);
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
            calls.load(Ordering::SeqCst),
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
        let calls = Arc::new(AtomicUsize::new(0));

        // src → sum(Disk) → print. print reads sum, so sum is the frontier the
        // run must load.
        let build = |calls: &Arc<AtomicUsize>| {
            let mut g = TestGraph::new();
            g.add("src", source(7, calls.clone()));
            g.add("sum", sum(CacheMode::Disk));
            g.add("print", |n| n.records());
            g.wire("src", 0, "sum", 0);
            g.wire("src", 0, "sum", 1);
            g.wire("sum", 0, "print", 0);
            g
        };

        let mut e = disk_engine(&dir, build(&calls));
        e.run_sinks().await;
        let after_run1 = calls.load(Ordering::SeqCst);

        // Reopen, then remove sum's blob before the run reaches it.
        let mut e = disk_engine(&dir, build(&calls));
        for entry in std::fs::read_dir(&dir.0).unwrap() {
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
            calls.load(Ordering::SeqCst) > after_run1,
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
        let runs = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(StdMutex::new(f64::NAN));

        // `produce` is a pure, Disk-persisted source whose declared output type
        // and value are `Int` or `Float`. Its id and inputs stay unchanged,
        // isolating output-signature invalidation.
        let build = |as_float: bool, runs: &Arc<AtomicUsize>, received: &Arc<StdMutex<f64>>| {
            let (runs, received) = (runs.clone(), received.clone());
            let mut g = TestGraph::new();
            g.add("produce", move |n: NodeSpec| {
                let ty = if as_float {
                    DataType::Float
                } else {
                    DataType::Int
                };
                n.pure()
                    .cache(CacheMode::Disk)
                    .output(ty)
                    .compute(move |_| {
                        runs.fetch_add(1, Ordering::SeqCst);
                        if as_float {
                            StaticValue::Float(1.5)
                        } else {
                            StaticValue::Int(7)
                        }
                    })
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
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(*received.lock().unwrap(), 7.0);

        // Run 2 (Float): the Float output re-keys produce's digest away from the
        // Int blob's key, so it isn't found — produce recomputes as Float.
        let mut e = disk_engine(&dir, build(true, &runs, &received));
        e.run_sinks().await;
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2,
            "the Float output re-keys away from the stale Int blob, so produce recomputes"
        );
        assert_eq!(
            *received.lock().unwrap(),
            1.5,
            "consume receives the recomputed Float, never the stale Int"
        );
    }

    /// A persisted node whose cone contains an impure node has digest `None`, so
    /// it's never disk-cached even with `Disk` — on reopen it recomputes.
    #[tokio::test]
    async fn impure_cone_persist_node_is_not_disk_cached() {
        let dir = TempDir::new("impure-cone");
        let calls = Arc::new(AtomicUsize::new(0));
        let build = |calls: &Arc<AtomicUsize>| {
            let mut g = source_mult_print(CacheMode::Disk, 11, calls.clone());
            g.edit_func("src", |func| func.behavior = FuncBehavior::Impure);
            g
        };

        let mut e = disk_engine(&dir, build(&calls));
        e.run_sinks().await;

        // Reopen: mult must recompute — an impure cone has no digest, so it
        // never caches to disk.
        let mut e = disk_engine(&dir, build(&calls));
        let run = e.run_sinks().await;
        assert!(
            !run.cached().contains(&"mult"),
            "an impure-cone node must not be disk-cached"
        );
        assert!(run.ran().contains(&"mult"), "mult recomputes on reopen");
    }

    /// A RAM-only node (the default) is never written to disk even though its
    /// cone is reproducible — only `Disk` opts in — so on reopen it recomputes.
    #[tokio::test]
    async fn memory_persistence_node_is_not_disk_cached() {
        let dir = TempDir::new("memory-persist");
        let calls = Arc::new(AtomicUsize::new(0));
        let build = |calls: &Arc<AtomicUsize>| source_mult_print(CacheMode::Ram, 1, calls.clone());

        let mut e = disk_engine(&dir, build(&calls));
        e.run_sinks().await;

        // Reopen: fresh RAM, nothing on disk for mult ⇒ it recomputes.
        let mut e = disk_engine(&dir, build(&calls));
        let run = e.run_sinks().await;
        assert!(
            !run.cached().contains(&"mult"),
            "a RAM-only node must not be disk-cached"
        );
        assert!(run.ran().contains(&"mult"), "mult recomputes on reopen");
    }

    /// A persisted node whose blob is on disk but whose custom output type has
    /// *no registered codec* — a value written by a build that had the codec,
    /// reopened by one that doesn't — is not reused from disk. It recomputes
    /// rather than panicking during loading; with the codec it is served from
    /// disk instead.
    #[tokio::test]
    async fn missing_codec_skips_disk_cache_instead_of_panicking() {
        use std::any::Any;
        use std::fmt;

        use async_trait::async_trait;
        use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

        use crate::data::codec::error::CodecError;
        use crate::library::TypeEntry;
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

        // A pure, disk-persisted sink emitting a custom `Blob`. The type's codec
        // is registered only when `with_codec` — and the store takes its codecs
        // from this same library, so that one flag decides both.
        let build = |with_codec: bool, recompute: &Arc<AtomicUsize>| {
            let recompute = recompute.clone();
            let mut g = TestGraph::new();
            g.library.register_type(
                BLOB_TYPE,
                if with_codec {
                    TypeEntry::custom_with_codec("Blob", Arc::new(BlobCodec))
                } else {
                    TypeEntry::custom("Blob")
                },
            );
            g.add("make_blob", move |n: NodeSpec| {
                n.pure()
                    .sink()
                    .cache(CacheMode::Disk)
                    .output(DataType::Custom(BLOB_TYPE.into()))
                    .lambda(crate::async_lambda!(
                        move |Invocation { outputs, .. }| { counter = recompute.clone() } => {
                            counter.fetch_add(1, Ordering::SeqCst);
                            outputs[0] = DynamicValue::Custom(Arc::new(Blob(vec![9, 9, 9])));
                            Ok(())
                        }
                    ))
            });
            g
        };

        let dir = TempDir::new("missing-codec");
        let recompute = Arc::new(AtomicUsize::new(0));

        // Run 1 (codec present): computes and writes the Blob to disk.
        let mut e = disk_engine(&dir, build(true, &recompute));
        e.run_sinks().await;
        assert_eq!(recompute.load(Ordering::SeqCst), 1, "the cold run computes");

        // Reopen with the codec: served from disk, and the hydration decode
        // reaches the engine's own runtime context store.
        let mut e = disk_engine(&dir, build(true, &recompute));
        let run = e.run_sinks().await;
        assert_eq!(
            recompute.load(Ordering::SeqCst),
            1,
            "codec present ⇒ served from disk"
        );
        assert!(run.cached().contains(&"make_blob"));
        assert_eq!(
            e.engine
                .executor
                .ctx_manager
                .contexts
                .get(DECODE_PROBE)
                .decodes,
            1,
            "the hydration decode reached the engine's runtime context store"
        );

        // Reopen WITHOUT the codec: the blob is present but undecodable, so it
        // is not flagged available — recompute, no panic.
        let mut e = disk_engine(&dir, build(false, &recompute));
        let run = e.run_sinks().await;
        assert_eq!(
            recompute.load(Ordering::SeqCst),
            2,
            "a missing codec ⇒ recompute"
        );
        assert!(
            !run.cached().contains(&"make_blob"),
            "an undecodable blob is not a cache hit"
        );
        assert!(
            run.ran().contains(&"make_blob"),
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
    use crate::{FsPathConfig, FsPathMode};

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

    /// A unique temp directory removed on drop (the disk store root).
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

    /// How often each counted node ran, and what the sink last saw.
    #[derive(Debug, Default, Clone)]
    struct Observed {
        loads: Arc<AtomicUsize>,
        annotates: Arc<AtomicUsize>,
        captured: Arc<StdMutex<String>>,
    }

    impl Observed {
        fn loads(&self) -> usize {
            self.loads.load(Ordering::SeqCst)
        }
        fn annotates(&self) -> usize {
            self.annotates.load(Ordering::SeqCst)
        }
        fn captured(&self) -> String {
            self.captured.lock().unwrap().clone()
        }
    }

    fn any_path() -> DataType {
        DataType::FsPath(Arc::new(FsPathConfig::default()))
    }

    fn existing_file() -> DataType {
        DataType::FsPath(Arc::new(FsPathConfig::new(FsPathMode::ExistingFile)))
    }

    /// Reads the file its declared-`FsPath` input names, counting invocations.
    fn loader(observed: Observed) -> impl FnOnce(NodeSpec) -> NodeSpec {
        move |n: NodeSpec| {
            n.pure()
                .input(existing_file())
                .output(DataType::String)
                .lambda(async_lambda!(
                    move |Invocation { inputs, outputs, .. }| { loads = observed.loads.clone() } => {
                        loads.fetch_add(1, Ordering::SeqCst);
                        let path = inputs[0].as_fs_path().unwrap().to_string();
                        let text = std::fs::read_to_string(&path).map_err(InvokeError::external)?;
                        outputs[0] = StaticValue::String(text).into();
                        Ok(())
                    }
                ))
        }
    }

    /// The sink both fixtures share: records the received value's text.
    fn capture(observed: Observed) -> impl FnOnce(NodeSpec) -> NodeSpec {
        move |n: NodeSpec| {
            n.sink().input(DataType::Any).lambda(async_lambda!(
                move |Invocation { inputs, .. }| { captured = observed.captured.clone() } => {
                    *captured.lock().unwrap() = inputs[0].as_string().unwrap_or_default().to_string();
                    Ok(())
                }
            ))
        }
    }

    /// `make_path(const name) → load_text → annotate → capture`, the three pure
    /// nodes on `mode`.
    ///
    /// `make_path` is pure `String → FsPath`: a producer whose *own* digest does
    /// not track the file, like any path-computing node. `annotate` sits
    /// downstream of the late-stamped loader, so it proves the reach-time
    /// re-stamp cascades and downstream caches still hit.
    ///
    /// Declaration order fixes the node ids, so a reopened engine addresses the
    /// same slots.
    fn path_graph(data_path: &str, mode: CacheMode, observed: Observed) -> TestGraph {
        let mut g = TestGraph::new();
        g.add("make_path", |n| {
            n.pure()
                .cache(mode)
                .input(DataType::String)
                .output(any_path())
                .compute(|inputs| StaticValue::FsPath(inputs[0].as_string().unwrap().to_string()))
        });
        g.add("load_text", |n| loader(observed.clone())(n).cache(mode));
        g.add("annotate", |n| {
            let annotates = observed.annotates.clone();
            n.pure()
                .cache(mode)
                .input(DataType::String)
                .output(DataType::String)
                .compute(move |inputs| {
                    annotates.fetch_add(1, Ordering::SeqCst);
                    StaticValue::String(format!("[{}]", inputs[0].as_string().unwrap()))
                })
        });
        g.add("capture", capture(observed));
        g.constant("make_path", 0, data_path);
        g.wire("make_path", 0, "load_text", 0);
        g.wire("load_text", 0, "annotate", 0);
        g.wire("annotate", 0, "capture", 0);
        g
    }

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

        // `loader` failed for want of a path identity, `dependent` was skipped
        // for reading it, and the run itself still succeeded.
        let assert_unavailable = |run: &RunOutcome, loader: &str, dependent: &str| {
            assert!(
                matches!(
                    run.error(loader),
                    Some(RunError::ResourceUnavailable { .. })
                ),
                "the node declaring the path must fail: {:?}",
                run.error(loader),
            );
            assert!(
                matches!(run.error(dependent), Some(RunError::SkippedUpstream { .. })),
                "its dependent must skip as errored-upstream: {:?}",
                run.error(dependent),
            );
        };

        // A const path, known before the run: the sweep cannot stamp it, so the
        // node reaches its turn with no digest and re-stamps there.
        let mut g = TestGraph::new();
        g.add("load_text", loader(Observed::default()));
        g.add("capture", capture(Observed::default()));
        g.constant("load_text", 0, StaticValue::FsPath(data_path.clone()));
        g.wire("load_text", 0, "capture", 0);
        let mut e = TestEngine::over(g);

        lock(0o000);
        let run = e.run(RunSeeds::sinks()).await;
        lock(0o755);
        assert_unavailable(
            &run.expect("a per-node failure must not abort the run"),
            "load_text",
            "capture",
        );

        // The same file reached through a producer's value, known only at the
        // node's turn. Same outcome, same route.
        let mut e = TestEngine::over(path_graph(&data_path, CacheMode::None, Observed::default()));
        lock(0o000);
        let run = e.run(RunSeeds::sinks()).await;
        lock(0o755);
        assert_unavailable(
            &run.expect("a per-node failure must not abort the run"),
            "load_text",
            "annotate",
        );
    }

    /// The core regression: a path arriving over a **Bind** edge keys the loader
    /// on the file behind the *delivered value*. Editing the file re-keys and
    /// recomputes the loader (pre-fix the chain's digests never changed, so the
    /// stale decode was served forever), while an unchanged file still reuses
    /// the cache — the reach-time re-stamp keeps wired-path loaders cacheable
    /// instead of tainting them uncacheable.
    #[tokio::test]
    async fn wired_path_rekeys_loader_on_file_change() {
        let data = temp_file("ram");
        std::fs::write(&data.0, "v1").unwrap();
        let observed = Observed::default();
        let mut e = TestEngine::over(path_graph(
            &data.0.to_string_lossy(),
            CacheMode::Ram,
            observed.clone(),
        ));

        // Cold run: everything computes. The loader's pre-run digest is `None`
        // — the delivered value does not exist yet — so it re-stamps at reach
        // time and runs.
        e.run_sinks().await;
        assert_eq!((observed.loads(), observed.annotates()), (1, 1));
        assert_eq!(observed.captured(), "[v1]");

        // Unchanged file: the loader reuses its RAM value under the full digest
        // (producer port + live file identity), and its *downstream* — whose
        // digest folds the loader's — skips too.
        let run = e.run_sinks().await;
        assert_eq!(
            observed.loads(),
            1,
            "unchanged file ⇒ the loader stays cached"
        );
        assert_eq!(
            observed.annotates(),
            1,
            "downstream of the late-stamped loader skips compute on its hit"
        );
        assert_eq!(run.cached(), ["annotate", "load_text", "make_path"]);

        // Edit the file (different length ⇒ unambiguous identity change). The
        // loader re-keys off the delivered value's file identity and the change
        // propagates downstream — while the structural upstream stays a hit.
        std::fs::write(&data.0, "v2-longer").unwrap();
        let run = e.run_sinks().await;
        assert_eq!(
            observed.loads(),
            2,
            "a file edit re-keys the loader through the wired path"
        );
        assert_eq!(
            observed.annotates(),
            2,
            "the loader's new digest invalidates its downstream"
        );
        assert_eq!(
            observed.captured(),
            "[v2-longer]",
            "fresh content flows down"
        );
        assert_eq!(
            run.ran(),
            ["load_text", "annotate", "capture"],
            "the path producer itself stays cached — nothing structural changed"
        );
    }

    /// Disk persistence across a reopen with a wired path: the loader's blob is
    /// keyed under the delivered path's live identity, so a fresh engine reuses
    /// it while the file is unchanged — hydrating the on-disk path producer just
    /// to stamp — and recomputes once the file changes, while the producer
    /// itself stays a disk hit. The downstream `annotate` proves the re-stamp
    /// *cascade*: on reopen its own pre-run digest is `None` too (it folds the
    /// loader's), and its reach-time re-stamp lands on its blob — the whole
    /// tainted cone skips compute, not just the loader.
    #[tokio::test]
    async fn wired_path_disk_reuse_survives_reopen_until_file_changes() {
        let dir = TempDir::new("disk");
        let data = temp_file("disk-data");
        std::fs::write(&data.0, "v1").unwrap();
        let observed = Observed::default();
        let path = data.0.to_string_lossy().into_owned();

        // A fresh engine over an identically-declared graph: `TestGraph` mints
        // ids in declaration order, so the reopened engine addresses the very
        // slots the blobs were written under.
        let reopen = |observed: Observed| {
            let mut e = TestEngine::over(path_graph(&path, CacheMode::Disk, observed));
            e.attach_disk_store(dir.0.clone());
            e
        };

        // Cold run: computes and stores the blobs.
        let mut e = reopen(observed.clone());
        e.run_sinks().await;
        assert_eq!((observed.loads(), observed.annotates()), (1, 1));

        // Reopen, unchanged file: the loader is a disk hit under the re-stamped
        // digest, and so is its downstream — each re-stamped at reach time,
        // producer-first.
        let mut e = reopen(observed.clone());
        let run = e.run_sinks().await;
        assert_eq!(
            observed.loads(),
            1,
            "reopen with an unchanged file serves the loader from disk"
        );
        assert_eq!(
            observed.annotates(),
            1,
            "downstream of the late-stamped loader is a disk hit too"
        );
        assert_eq!(run.cached(), ["annotate", "load_text", "make_path"]);
        assert_eq!(
            observed.captured(),
            "[v1]",
            "the sink reads the hydrated disk value"
        );

        // Reopen after an edit: the loader's key moved ⇒ recompute, propagating
        // downstream; the path producer's own digest is unchanged, so it stays a
        // disk hit feeding the recompute.
        std::fs::write(&data.0, "v2-longer").unwrap();
        let mut e = reopen(observed.clone());
        let run = e.run_sinks().await;
        assert_eq!(
            observed.loads(),
            2,
            "reopen after a file edit recomputes the loader"
        );
        assert_eq!(
            observed.annotates(),
            2,
            "the loader's new digest invalidates its downstream"
        );
        assert_eq!(observed.captured(), "[v2-longer]");
        assert_eq!(
            run.ran(),
            ["load_text", "annotate", "capture"],
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
