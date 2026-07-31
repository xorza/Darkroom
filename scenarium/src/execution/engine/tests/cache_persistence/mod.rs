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

mod blob_recovery;
mod cache_modes;
mod frontier;
