//! Node-graph execution as an explicit three-phase pipeline:
//!
//! 1. **compile** — [`ExecutionEngine::update`] flattens the authoring `Graph`
//!    into an immutable [`ExecutionProgram`](program::ExecutionProgram).
//! 2. **plan** — the [`Planner`](planner::Planner) turns the program + current
//!    cache state into an [`ExecutionPlan`](plan::ExecutionPlan) (the schedule).
//! 3. **execute** — the [`Executor`](executor::Executor) runs the plan,
//!    invoking each scheduled node and updating its runtime cache.
//!
//! [`ExecutionEngine`] owns every piece (program, plan, planner, the cross-run
//! cache, and executor) and exposes `update` (phase 1) and `execute` (phases
//! 2–3, run back-to-back).

use common::CancelToken;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;

use crate::data::DynamicValue;
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
use program::{ExecutionNode, ExecutionProgram};

// === Error Types ===

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum Error {
    #[error("{message}")]
    Invoke { func_id: FuncId, message: String },
    #[error("invalid graph: {message}")]
    InvalidGraph { message: String },
    #[error("node {func_id:?} was cancelled before completing")]
    Cancelled { func_id: FuncId },
    #[error("Cycle detected while building execution graph at node {node_id:?}")]
    CycleDetected { node_id: NodeId },
    #[error("event lambda for node {node_id:?} panicked: {message}")]
    EventLambdaPanic { node_id: NodeId, message: String },
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
    flattener: flatten::Flattener,
    /// How the last `update` flattened the graph (authoring↔execution id
    /// map). Rebuilt each compile, cloned into each run's `ExecutionStats`
    /// so the editor can project stats onto its nodes. Compile scratch,
    /// not part of the serialized program.
    flatten_map: FlattenMap,
    /// Per-node cross-run cache (output values, digests, node state), reconciled
    /// to the node set at each `update`.
    cache: Cache,
    executor: Executor,
    planner: Planner,
    /// Reusable plan buffer, recycled across runs to avoid reallocation.
    plan: ExecutionPlan,
    /// Output cache: flags which `persist` (content-addressed) and `CachePassthrough`
    /// (explicit-path) node outputs are available on disk at `update` (so the planner
    /// prunes), reads only the ones a run consumes into RAM at execute time, and
    /// persists fresh ones after a run. Holds the one codec registry and the optional
    /// disk root; empty default is memory-only. Set via [`Self::set_output_cache`].
    output_cache: OutputCache,
}

impl ExecutionEngine {
    // === Accessors ===

    pub(crate) fn by_id(&self, node_id: &NodeId) -> Option<&ExecutionNode> {
        self.program.e_nodes.by_key(node_id)
    }

    pub fn is_empty(&self) -> bool {
        self.program.e_nodes.is_empty()
    }

    // === State Management ===

    pub fn clear(&mut self) {
        self.program.clear();
        self.plan.clear();
        self.cache.clear();
        self.flatten_map.reset();
    }

    pub fn reset_states(&mut self) {
        self.cache.reset_states();
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
            &mut self.flatten_map,
        ) as usize;

        // Realign the runtime cache to the rebuilt node set (preserve by id,
        // default new, trim gone).
        self.cache.reconcile(&self.program.e_nodes);

        validate::compiled(&self.program, &self.cache, library);

        // A node's digest changes only at compile (consts/bindings/func versions
        // are fixed between updates), so recompute it now and pull any disk-cached
        // `persist` outputs into RAM — both off the per-execute path. The trade-off
        // is that an external file change with no graph edit isn't noticed until
        // the next update/reopen.
        // Recompute the compile-stable per-node columns: resolved output types (fed
        // into the digest and the disk cache's codec check) and content digests. Both
        // are derived from the full library — every func is present (`check_with`
        // validated them) — and off the per-execute path.
        self.cache.recompute_digests(&self.program, library);
        // Flag which cached outputs (content-addressed `persist` + explicit-path
        // `CachePassthrough`) are available on disk for the current digest, so the
        // planner prunes their cones — *without* reading them. The bytes load lazily
        // at execute time, and only for the values a run actually consumes.
        self.output_cache
            .mark_available(&self.program, &mut self.cache);
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
        // Phase 2 + 2b: schedule into the reusable plan buffer (node digests were
        // recomputed at `update` and persist across re-executes), then read the
        // disk-cached values the schedule will consume (the frontier feeding
        // executing nodes) into RAM — values behind a pruned producer stay on disk.
        // A frontier blob that fails to load clears its own availability, so we
        // re-plan to schedule that node for recompute instead of pruning it behind an
        // absent value; each failure clears one flag, so this converges.
        loop {
            self.planner
                .plan(&self.program, &self.cache, &seeds, &mut self.plan)?;
            if self
                .output_cache
                .hydrate_frontier(&self.program, &self.plan, &mut self.cache)
            {
                break;
            }
        }

        // Phase 3: run the schedule.
        let mut stats = self
            .executor
            .run(
                &self.program,
                &self.plan,
                &mut self.cache,
                &self.flatten_map,
                progress,
                cancel,
            )
            .await;

        // Phase 3b: persist freshly-computed cache outputs to their files.
        self.output_cache
            .store(
                &self.program,
                &self.plan,
                &self.cache,
                &mut self.executor.ctx_manager,
            )
            .await;

        // Phase 3c: reclaim RAM from prior-run values this run left untouched and
        // that the disk store (just written above) can serve again on demand.
        self.output_cache
            .evict_unused(&self.program, &self.plan, &mut self.cache);

        stats.triggered_events = seeds.events;
        // Annotate with how the graph was flattened so the stats' flat ids
        // can be projected back onto authoring nodes (the executor itself
        // stays oblivious to the authoring graph).
        stats.flatten = self.flatten_map.clone();

        Ok(stats)
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
        self.planner
            .plan(&self.program, &self.cache, &seeds, &mut self.plan)
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
        self.plan.verdicts[idx]
    }

    pub(crate) fn node_output_usage(&self, e_node: &ExecutionNode) -> &[u32] {
        &self.plan.output_usage[e_node.outputs.range()]
    }

    pub(crate) fn runtime_slot(&self, e_node: &ExecutionNode) -> &cache::RuntimeSlot {
        self.cache.slots.by_key(&e_node.id).unwrap()
    }

    /// Iterator over runtime slots, index-aligned to `e_nodes`.
    pub(crate) fn runtime_slots(&self) -> std::slice::Iter<'_, cache::RuntimeSlot> {
        self.cache.slots.iter()
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
        slot.output_values = Some(values);
        slot.output_digest = slot.current_digest;
    }
}
