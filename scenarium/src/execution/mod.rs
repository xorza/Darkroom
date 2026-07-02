//! Node-graph execution as an explicit three-phase pipeline:
//!
//! 1. **compile** — [`ExecutionEngine::update`] flattens the authoring `Graph`
//!    into an immutable [`ExecutionProgram`](program::ExecutionProgram).
//! 2. **plan** — the [`Planner`](planner::Planner) turns the program into an
//!    [`ExecutionPlan`](plan::ExecutionPlan) (the schedule). Purely structural —
//!    reachability + topological order + output usage, no cache/digest state.
//! 3. **execute** — the [`Executor`](executor::Executor) walks the schedule
//!    producer-first, computing each node's content digest and deciding reuse
//!    (RAM / disk) or recompute inline, then updating its runtime cache.
//!
//! [`ExecutionEngine`] owns every piece (program, plan, planner, the cross-run
//! cache, and executor) and exposes `update` (phase 1) and `execute` (phases
//! 2–3, run back-to-back).

use std::sync::Arc;

use common::CancelToken;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;

use crate::data::DynamicValue;
use crate::execution::flatten::Flattener;
use crate::execution_stats::{ExecutionStats, FlattenMap, RunProgress};
use crate::graph::{Graph, NodeId};
use crate::library::Library;
use crate::prelude::FuncId;

pub(crate) mod blob;
pub(crate) mod cache;
pub(crate) mod digest;
pub(crate) mod event;
pub(crate) mod executor;
mod flatten;
pub(crate) mod output_cache;
pub(crate) mod plan;
pub(crate) mod planner;
pub(crate) mod program;
mod query;
#[cfg(test)]
mod tests;
pub(crate) mod validate;

use cache::Cache;
use event::EventRef;
use executor::Executor;
use output_cache::OutputCache;
use plan::ExecutionPlan;
use planner::Planner;
#[cfg(test)]
use program::ExecutionNode;
use program::ExecutionProgram;

// === Error Types ===

/// An **operation-level** failure that aborts a whole compile / plan / run: the graph
/// won't compile ([`InvalidGraph`](Error::InvalidGraph)), the schedule has a cycle
/// ([`CycleDetected`](Error::CycleDetected)), or the event loop's lambda panicked
/// ([`EventLambdaPanic`](Error::EventLambdaPanic)). It's the error type of the engine's
/// `Result`-returning entry points. A *single node's* run failure is a [`RunError`]
/// (collected into [`ExecutionStats::node_errors`](crate::execution_stats::ExecutionStats)),
/// never one of these — the two phases can't be confused at the type level.
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum Error {
    #[error("invalid graph: {message}")]
    InvalidGraph { message: String },
    #[error("Cycle detected while building execution graph at node {node_id:?}")]
    CycleDetected { node_id: NodeId },
    #[error("event lambda for node {node_id:?} panicked: {message}")]
    EventLambdaPanic { node_id: NodeId, message: String },
}

/// A **single node's** run-time failure, collected per-node into
/// [`ExecutionStats::node_errors`](crate::execution_stats::ExecutionStats). Distinct
/// from [`Error`] (whole-operation failures): a `RunError` always concerns exactly one
/// node, so it can't carry a compile/plan failure, and a caller reading `node_errors`
/// can't mistake a setup failure for a node's outcome.
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum RunError {
    #[error("{message}")]
    Invoke { func_id: FuncId, message: String },
    #[error("node {func_id:?} skipped: an upstream dependency errored")]
    SkippedUpstream { func_id: FuncId },
    #[error("node {func_id:?} was cancelled before completing")]
    Cancelled { func_id: FuncId },
}

pub type Result<T> = std::result::Result<T, Error>;

// === Value Types ===

#[derive(Debug, Default)]
pub struct ArgumentValues {
    pub inputs: Vec<Option<DynamicValue>>,
    pub outputs: Vec<DynamicValue>,
}

/// What seeds a run's schedule — the roots the planner walks back from. The three
/// are independent and combine: a run can target terminal nodes, the event loop's
/// triggerable events, and/or a set of injected events, all at once.
#[derive(Debug, Default, Clone)]
pub struct RunSeeds {
    /// Include all terminal nodes — the ordinary "produce the outputs" trigger.
    pub terminals: bool,
    /// Include every node owning a subscribed event — drives the event loop.
    pub event_triggers: bool,
    /// Run the subscribers of these specific fired events.
    pub events: Vec<EventRef>,
}

// === Execution Engine ===

/// The three-phase pipeline container. Owns the compiled `program`, the
/// `flattener` (compile scratch) and its `flatten` map (flat↔authoring ids), the
/// reusable `plan` buffer, the `planner` (scheduling scratch), the cross-run
/// `cache` (per-node outputs + state), the `executor` (run loop + context), and
/// the `output_cache` (file persistence). Not serializable — the persistent form
/// is the [`ExecutionProgram`] alone.
#[derive(Debug, Default)]
pub struct ExecutionEngine {
    pub(crate) program: ExecutionProgram,
    /// Reusable subgraph-flattening scratch (kept across compiles).
    flattener: Flattener,
    /// How the last `update` flattened the graph (authoring↔execution id map).
    /// Rebuilt each compile via `Arc::make_mut` (no clone when no prior run's stats
    /// still hold it), and handed to each run's `ExecutionStats` by refcount bump.
    /// Compile scratch, not part of the serialized program.
    flatten_map: Arc<FlattenMap>,
    /// Per-node cross-run cache (output values, digests, node state), reconciled
    /// to the node set at each `update`.
    cache: Cache,
    executor: Executor,
    planner: Planner,
    /// Reusable plan buffer, recycled across runs to avoid reallocation.
    plan: ExecutionPlan,
    /// Output cache: at execution time serves `persist` (content-addressed) and
    /// `CachePassthrough` (explicit-path) nodes from disk when a blob for their digest
    /// exists, reads only the ones a run consumes into RAM, and persists fresh ones after
    /// a run. Holds the one codec registry and the optional disk root; empty default is
    /// memory-only. Set via [`Self::set_output_cache`].
    output_cache: OutputCache,
}

impl ExecutionEngine {
    // === Accessors ===

    pub fn is_empty(&self) -> bool {
        self.program.e_nodes.is_empty()
    }

    // === State Management ===

    pub fn clear(&mut self) {
        self.program.clear();
        self.plan.clear();
        self.cache.clear();
        Arc::make_mut(&mut self.flatten_map).reset();
    }

    /// Swap the [`OutputCache`] — the library snapshot (its type table supplies
    /// the custom-value codecs) plus the optional
    /// content-addressed store root. At the next `update`, `persist` (content-
    /// addressed) and `CachePassthrough` (explicit-path) outputs hydrate from their
    /// files on a hit (skipping recompute), and freshly-computed ones are stored
    /// after a run. The RAM cache is keyed by node id + digest, independent of the
    /// root, so swapping keeps any warm in-memory outputs.
    pub fn set_output_cache(&mut self, cache: OutputCache) {
        self.output_cache = cache;
    }

    // === Phase 1: compile ===

    pub fn update(&mut self, graph: &Graph, library: &Library) -> Result<()> {
        // Validate the graph against the library before touching any state.
        // The graph+library pair is untrusted input here (a document can be
        // stale against an evolved library — a dropped func, a shrunk port
        // list), so an invalid one is a recoverable error the caller
        // surfaces, not a logic bug. Checking first leaves the prior program
        // intact on error and lets the flatten pass resolve every reference
        // infallibly.
        if let Err(e) = graph.check_with(library) {
            tracing::error!(error = %e, "graph update rejected: invalid graph");
            return Err(Error::InvalidGraph {
                message: e.to_string(),
            });
        }

        // The plan isn't cleared here: every `execute` re-`plan`s from scratch (the
        // planner `reset`s the buffer), and nothing reads the plan between a compile
        // and the next run. `clear()` is reserved for full teardown (`Self::clear`).

        // Flatten subgraphs straight into execution nodes — no intermediate
        // `Graph`. Everything below is boundary-agnostic (func nodes only).
        self.program.n_outputs = self.flattener.build(
            &mut self.program.e_nodes,
            flatten::Pools {
                inputs: &mut self.program.inputs,
                events: &mut self.program.events,
            },
            graph,
            library,
            Arc::make_mut(&mut self.flatten_map),
        ) as usize;

        // Realign the runtime cache to the rebuilt node set (preserve by id,
        // default new, trim gone).
        self.cache.reconcile(&self.program.e_nodes);

        // Resolve the program's output-type pool from the full library (every func is
        // present — `check_with` validated them), making the compiled program
        // self-describing. Fed into the digest below (an output-signature change
        // re-keys) and the disk cache's codec check, with no library at run time.
        self.program.resolve_output_types(library);

        validate::compiled(&self.program, &self.cache, library);
        Ok(())
    }

    // === Phases 2–3: plan then execute ===

    /// When `progress` is `Some`, a [`RunProgress`] is sent before and after
    /// each node's lambda runs, for live per-node feedback ahead of the final
    /// stats. When `cancel` is `Some` and gets set mid-run, scheduling stops
    /// after the in-flight node and the returned stats are marked `cancelled`.
    pub async fn execute(
        &mut self,
        seeds: RunSeeds,
        progress: Option<&UnboundedSender<RunProgress>>,
        cancel: CancelToken,
    ) -> Result<ExecutionStats> {
        // Phase 2: schedule into the reusable plan buffer. Purely structural now —
        // reachability + topological order + output usage, no cache/digest state. Every
        // reachable node is scheduled; the executor computes each node's digest as it
        // reaches it (producers first) and decides reuse-from-RAM / load-from-disk /
        // recompute inline, so the disk frontier is materialized lazily during the run.
        self.planner.plan(&self.program, &seeds, &mut self.plan)?;

        // Phase 3: run the schedule. Each node's disk cache is written the moment it
        // finishes (inside the run loop), not batched here — so a long run's earlier
        // caches are durable even if a later node fails or the run is cancelled.
        let mut stats = self
            .executor
            .run(
                &self.program,
                &self.plan,
                &mut self.cache,
                &self.output_cache,
                &self.flatten_map,
                progress,
                cancel,
            )
            .await;

        // Phase 3b: reclaim RAM from prior-run values this run left untouched and
        // that the disk store (written per-node above) can serve again on demand.
        let executor = &self.executor;
        self.output_cache
            .evict_unused(&self.program, &self.plan, &mut self.cache, |idx| {
                executor.ran(idx)
            });

        stats.triggered_events = seeds.events;
        // Annotate with how the graph was flattened so the stats' flat ids
        // can be projected back onto authoring nodes (the executor itself
        // stays oblivious to the authoring graph).
        stats.flatten = self.flatten_map.clone();

        Ok(stats)
    }

    /// Persist to disk any **content-addressed** (`persist`) node that holds a resident
    /// value but isn't on disk yet — e.g. a node just toggled to
    /// `CachePersistence::Disk` whose value is still in RAM from a prior run. The worker
    /// calls this on `SaveCaches`, since such a node is a cache hit and so never
    /// re-executes to store itself.
    ///
    /// Never overwrites identical content: a content-addressed blob's path *is* its
    /// content hash, so [`OutputCache::store_node`] skips it when it already exists.
    /// Explicit-path (`CachePassthrough`) nodes are deliberately excluded here — their
    /// file is (re)written by their own execution, so flushing them would overwrite an
    /// identical file. Also a no-op for a node with no resident value.
    pub async fn store_resident_caches(&mut self) {
        for idx in self.program.node_indices() {
            if !self.program.e_nodes[idx].persist {
                continue;
            }
            self.output_cache
                .store_node(
                    &self.program,
                    idx,
                    &self.cache,
                    &mut self.executor.ctx_manager,
                )
                .await;
        }
    }
}

/// Test-only inspection of the last plan's per-run flags and the runtime
/// slots. Nothing in production reads per-run state off the engine — the
/// executor reads it straight from the live `ExecutionPlan`.
#[cfg(test)]
impl ExecutionEngine {
    pub(crate) async fn execute_terminals(&mut self) -> Result<ExecutionStats> {
        self.execute(
            RunSeeds {
                terminals: true,
                ..Default::default()
            },
            None,
            CancelToken::never(),
        )
        .await
    }

    pub(crate) async fn execute_events<T: IntoIterator<Item = EventRef>>(
        &mut self,
        events: T,
    ) -> Result<ExecutionStats> {
        self.execute(
            RunSeeds {
                events: events.into_iter().collect(),
                ..Default::default()
            },
            None,
            CancelToken::never(),
        )
        .await
    }

    /// Run only the planning phase (no execution), leaving the schedule in
    /// `self.plan` for inspection.
    pub(crate) fn prepare_execution(
        &mut self,
        terminals: bool,
        event_triggers: bool,
        events: &[EventRef],
    ) -> Result<()> {
        let seeds = RunSeeds {
            terminals,
            event_triggers,
            events: events.to_vec(),
        };
        self.planner.plan(&self.program, &seeds, &mut self.plan)
    }

    pub(crate) fn by_id(&self, node_id: &NodeId) -> Option<&ExecutionNode> {
        self.program.e_nodes.by_key(node_id)
    }

    pub(crate) fn by_name(&self, node_name: &str) -> Option<&ExecutionNode> {
        self.program
            .e_nodes
            .iter()
            .find(|node| node.name == node_name)
    }

    pub(crate) fn node_inputs(&self, e_node: &ExecutionNode) -> &[program::ExecutionInput] {
        self.program.node_inputs(e_node)
    }

    pub(crate) fn node_events(&self, e_node: &ExecutionNode) -> &[program::ExecutionEvent] {
        &self.program.events[e_node.events.range()]
    }

    pub(crate) fn node_verdict(&self, e_node: &ExecutionNode) -> plan::NodeVerdict {
        let idx = self.program.e_nodes.index_of_key(&e_node.id).unwrap();
        self.plan.verdicts[idx.into()]
    }

    pub(crate) fn node_output_usage(&self, e_node: &ExecutionNode) -> &[u32] {
        &self.plan.output_usage[e_node.outputs.range()]
    }

    /// Whether node `idx` recomputed (rather than reused a cache) in the last run.
    pub(crate) fn node_ran(&self, idx: program::NodeIdx) -> bool {
        self.executor.ran(idx)
    }

    pub(crate) fn runtime_slot(&self, e_node: &ExecutionNode) -> &cache::RuntimeSlot {
        self.cache.slots.by_key(&e_node.id).unwrap()
    }

    /// Seed a node's cached output (simulating a prior run): set the value and
    /// stamp `output_digest` from the current digest, so the planner sees a hit.
    pub(crate) fn set_output_values(&mut self, node_name: &str, values: Vec<DynamicValue>) {
        let idx = self
            .program
            .e_nodes
            .index_of_key(&self.by_name(node_name).unwrap().id);
        let idx = idx.unwrap();
        let slot = &mut self.cache.slots[idx];
        slot.value = cache::ValueCache::Resident {
            values,
            produced_under: slot.current_digest,
        };
    }
}
