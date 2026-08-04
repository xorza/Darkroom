use super::*;

use ::common::TempDir;

use crate::execution::cache::runtime::error::CacheNodeError;
use crate::execution::schedule::NodeState;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// `src → mult(mode) → print`, both of mult's inputs fed by the source.
/// The sink is impure, so `mult` is demanded every run.
fn source_mult_print(mode: CacheMode, value: i64, calls: &Calls) -> TestGraph {
    let mut g = TestGraph::new();
    g.add("src", |n| n.counted(value, calls));
    g.add("mult", |n| n.mult().cache(mode));
    g.add("print", |n| n.records());
    g.wire("src", 0, "mult", 0);
    g.wire("src", 0, "mult", 1);
    g.wire("mult", 0, "print", 0);
    g
}

mod blob_recovery;
mod cache_modes;
mod frontier;
