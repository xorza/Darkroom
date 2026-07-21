//! Node-graph execution as an explicit three-phase pipeline:
//!
//! 1. **compile** — the [`Compiler`](compile::Compiler) flattens the authoring
//!    `Graph` into an immutable [`ExecutionProgram`](program::ExecutionProgram).
//!    Runs on the *host's* thread (compile errors are synchronous); the resulting
//!    [`CompiledGraph`](compile::CompiledGraph) is installed into the engine
//!    via [`ExecutionEngine::install`], which cannot fail.
//! 2. **plan** — the [`Planner`](plan::Planner) turns the program into an
//!    [`ExecutionPlan`](plan::ExecutionPlan) (the schedule). Purely structural —
//!    reachability + topological order + missing-input verdicts, no cache/digest state.
//! 3. **execute** — [`RunResourceStamps`] prepares external identities on the blocking
//!    pool; the [`Resolver`](resolve::Resolver) stamps content digests, then derives
//!    cache-aware liveness, exact output demand, and reader counts in one consumer-first
//!    sweep. The [`Executor`] walks the surviving schedule producer-first.
//!
//! [`ExecutionEngine`] owns the run-side pieces (program, plan, planner, the
//! cross-run cache, and executor) and exposes `install` (phase 1's artifact)
//! and `execute` (phases 2–3, run back-to-back).

use std::ops::{Index, IndexMut};

use common::{CancelToken, Span};
use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;

#[cfg(test)]
use crate::DynamicValue;
use crate::execution::compile::CompiledGraph;
use crate::execution::identity::NodeAddress;
use crate::execution::report::RunEvent;
use crate::execution::stats::ExecutionStats;
use crate::graph::NodeId;
use crate::node::definition::FuncId;
#[cfg(test)]
use crate::node::lambda::OutputDemand;

pub(crate) mod cache;
pub(crate) mod codec;
pub(crate) mod compile;
pub(crate) mod digest;
pub(crate) mod disk_store;
pub(crate) mod event;
pub(crate) mod executor;
mod flatten;
pub(crate) mod identity;
pub(crate) mod plan;
pub(crate) mod program;
mod query;
pub(crate) mod report;
pub(crate) mod resolve;
pub(crate) mod resource;
pub(crate) mod stats;
#[cfg(test)]
mod tests;
pub(crate) mod validate;

use cache::RuntimeCache;
use disk_store::DiskStore;
use event::EventRef;
use executor::Executor;
use plan::{ExecutionPlan, Planner};
#[cfg(test)]
use program::ExecutionNode;
use program::OutputIdx;
use resolve::Resolver;
use resource::RunResourceStamps;

/// A column aligned to the program's flat output pool. Node-local views are sliced by
/// their compiled output span, while individual entries require an [`OutputIdx`].
#[derive(Debug, Clone, Default)]
pub(crate) struct OutputColumn<T> {
    pub(crate) values: Vec<T>,
}

impl<T: Clone> OutputColumn<T> {
    pub(crate) fn reset(&mut self, len: usize, value: T) {
        self.values.clear();
        self.values.resize(len, value);
    }
}

impl<T> From<Vec<T>> for OutputColumn<T> {
    fn from(values: Vec<T>) -> Self {
        Self { values }
    }
}

impl<T> Index<OutputIdx> for OutputColumn<T> {
    type Output = T;

    fn index(&self, index: OutputIdx) -> &T {
        &self.values[index.idx()]
    }
}

impl<T> IndexMut<OutputIdx> for OutputColumn<T> {
    fn index_mut(&mut self, index: OutputIdx) -> &mut T {
        &mut self.values[index.idx()]
    }
}

pub(crate) type NodeMap<T> = HashMap<NodeId, T>;
pub(crate) type NodeSet = HashSet<NodeId>;

fn reset_node_map<T: Clone>(
    map: &mut NodeMap<T>,
    node_ids: impl Iterator<Item = NodeId>,
    value: T,
) {
    map.clear();
    map.extend(node_ids.map(|node_id| (node_id, value.clone())));
}

impl<T> OutputColumn<T> {
    pub(crate) fn slice(&self, outputs: Span) -> &[T] {
        &self.values[outputs.range()]
    }

    pub(crate) fn slice_mut(&mut self, outputs: Span) -> &mut [T] {
        &mut self.values[outputs.range()]
    }
}

/// An **operation-level** failure that aborts a whole plan / run: the schedule has a
/// cycle ([`CycleDetected`](Error::CycleDetected)), a node seed didn't resolve
/// ([`NodeSeedNotFound`](Error::NodeSeedNotFound)), or the event loop's lambda panicked
/// ([`EventLambdaPanic`](Error::EventLambdaPanic)). It's the error type of the engine's
/// `Result`-returning entry points. A *single node's* run failure is a [`RunError`]
/// (collected into [`ExecutionStats::node_errors`](crate::execution::stats::ExecutionStats)),
/// never one of these; a graph that won't compile is a
/// [`CompileError`](compile::CompileError), produced on the host before anything
/// reaches the engine — the phases can't be confused at the type level.
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum Error {
    #[error("Cycle detected while building execution graph at node {node_id:?}")]
    CycleDetected { node_id: NodeId },
    /// A node seed didn't resolve against the compiled program. Seeds are batched with
    /// the graph they target, so a miss means inconsistent caller state (a deleted,
    /// stale, composite, or boundary target) — the run fails rather than silently
    /// skipping the seed.
    #[error("node seed {address:?} not found in the compiled program")]
    NodeSeedNotFound { address: NodeAddress },
    #[error("event lambda for node {node_id:?} panicked: {message}")]
    EventLambdaPanic { node_id: NodeId, message: String },
}

/// A **single node's** run-time failure, collected per-node into
/// [`ExecutionStats::node_errors`](crate::execution::stats::ExecutionStats). Distinct
/// from [`Error`] (whole-operation failures): a `RunError` always concerns exactly one
/// node, so it can't carry a compile/plan failure, and a caller reading `node_errors`
/// can't mistake a setup failure for a node's outcome.
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum RunError {
    #[error("{message}")]
    Invoke { func_id: FuncId, message: String },
    // The messages omit `func_id` (kept as machine-readable data): a `RunError`
    // is already paired with its `NodeId` in `node_errors`, so these surface to
    // the editor attributed to the node — a raw id in the text would be noise.
    /// The node's func was registered without an implementation
    /// ([`FuncLambda::None`](crate::node::lambda::FuncLambda)), so the node
    /// can't execute. A host/library configuration error, reported per-node
    /// (its consumers skip as errored-upstream) rather than crashing the run.
    #[error("the node's function has no implementation attached")]
    MissingLambda { func_id: FuncId },
    #[error("skipped: an upstream dependency errored")]
    SkippedUpstream { func_id: FuncId },
    /// A bound input's disk-cached value failed to load (corrupt/deleted blob).
    /// Distinct from [`SkippedUpstream`](Self::SkippedUpstream): no upstream node
    /// holds an error to point at. The bad blob is deleted on the failed read, so
    /// the producer recomputes next run. `input` is the consumer's input position.
    #[error("skipped: a cached input failed to load from disk (recomputes next run)")]
    InputLoadFailed { func_id: FuncId, input: usize },
    #[error("demanded outputs {outputs:?} were left unbound")]
    OutputsNotProduced {
        func_id: FuncId,
        outputs: Vec<usize>,
    },
    #[error("cancelled before completing")]
    Cancelled { func_id: FuncId },
}

pub type Result<T> = std::result::Result<T, Error>;

/// What seeds a run's schedule — the roots the planner walks back from. The four
/// are independent and combine: a run can target sink nodes, the event loop's
/// triggerable events, a set of injected events, and/or specific nodes, all at once.
#[derive(Debug, Default, Clone)]
pub(crate) struct RunSeeds {
    /// Include all sink nodes — the ordinary "produce the outputs" trigger.
    pub sinks: bool,
    /// Include every node owning a subscribed event — drives the event loop.
    pub event_triggers: bool,
    /// Run the subscribers of these specific fired events.
    pub events: Vec<EventRef>,
    /// Run the cones of these specific nodes (authoring ids) and deliver every output —
    /// the on-demand "run to this node" / preview trigger. The
    /// worker batches these with the graph they target. An explicitly seeded disabled
    /// node is enabled for this run; an id that doesn't resolve against the compiled
    /// program fails with [`Error::NodeSeedNotFound`].
    pub nodes: Vec<NodeAddress>,
}

/// The run-side pipeline container. Owns the installed `program` and its
/// `flatten_map` (flat↔authoring ids), the reusable `plan` buffer, the `planner`
/// (scheduling scratch), the cross-run `cache` (per-node outputs + state, plus its
/// owned `DiskStore` file persistence and the caching policy), and the `executor`
/// (run loop + context). Compilation happens on the host ([`compile::Compiler`]);
/// the engine only ever receives ready [`CompiledGraph`]s. Not serializable — the
/// persistent form is the [`ExecutionProgram`] alone.
#[derive(Debug, Default)]
pub(crate) struct ExecutionEngine {
    /// The installed compile artifact: the program plus its flatten map
    /// (authoring↔execution id map, resolving node seeds at plan time).
    /// Replaced wholesale by [`Self::install`].
    pub(crate) compiled: CompiledGraph,
    /// Per-node cross-run cache (output values, digests, node state) plus the [`DiskStore`]
    /// backing it and the caching policy over both — reuse, hydration, persistence, eviction.
    /// The RAM slots are reconciled to the node set at each `install`; the disk store is set
    /// via [`Self::set_disk_store`] and kept across installs.
    cache: RuntimeCache,
    executor: Executor,
    planner: Planner,
    /// Cache-aware refinement of the plan: resolves reuse + cuts cones feeding only cache
    /// hits, between plan and execute. Owns reusable per-run scratch (see `resolve.rs`).
    resolver: Resolver,
    /// Per-run external-resource identities, collected off-thread and shared by initial
    /// resolution and late bound-resource restamps.
    resource_stamps: RunResourceStamps,
    /// Reusable plan buffer, recycled across runs to avoid reallocation.
    plan: ExecutionPlan,
}

impl ExecutionEngine {
    pub(crate) fn is_empty(&self) -> bool {
        self.compiled.program.e_nodes.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.compiled = CompiledGraph::default();
        self.plan.clear();
        self.cache.clear();
        self.resource_stamps = RunResourceStamps::default();
    }

    /// Swap the [`DiskStore`] — the library snapshot (its type table supplies
    /// the custom-value codecs) plus the optional
    /// store root. At the next `install`, `persist` outputs hydrate
    /// from their blobs on a hit (skipping recompute), and freshly-computed ones are
    /// stored after a run. The RAM cache is keyed by node id + digest, independent of the
    /// root, so swapping keeps any warm in-memory outputs.
    pub(crate) fn set_disk_store(&mut self, disk_store: DiskStore) {
        self.cache.set_disk_store(disk_store);
    }

    /// Install a host-compiled [`CompiledGraph`] as the current program.
    /// Infallible: everything that can go wrong went wrong at compile
    /// ([`compile::Compiler`]), on the host's thread.
    ///
    /// The plan isn't cleared here: every `execute` re-`plan`s from scratch (the
    /// planner `reset`s the buffer), and nothing reads the plan between an install
    /// and the next run. `clear()` is reserved for full teardown (`Self::clear`).
    pub(crate) fn install(&mut self, compiled: CompiledGraph) {
        self.compiled = compiled;

        // Realign the runtime cache to the new node set (preserve by id,
        // default new, trim gone).
        self.cache.reconcile(&self.compiled.program);

        self.compiled.validate_installed_debug(&self.cache);
    }

    /// When `events` is `Some`, a [`RunEvent`] is sent for live per-node
    /// feedback ahead of the final stats: a `RunEvent::Progress` before and
    /// after each node's lambda runs, and a `RunEvent::PinnedOutputs` when a
    /// node with a pinned output (or that is itself a pinned root) produces or
    /// reuses its value, so a GUI preview updates without polling. When
    /// `cancel` is `Some` and gets set mid-run, scheduling stops after the
    /// in-flight node and the returned stats are marked `cancelled`.
    pub(crate) async fn execute(
        &mut self,
        seeds: RunSeeds,
        events: Option<&UnboundedSender<RunEvent>>,
        cancel: CancelToken,
    ) -> Result<ExecutionStats> {
        // Phase 2: schedule into the reusable plan buffer. Purely structural —
        // reachability + topological order + missing-input verdicts + walk roots, no
        // cache/digest state. The flatten map resolves authoring node seeds to flat roots.
        self.planner.plan(&self.compiled, &seeds, &mut self.plan)?;

        // Phase 2a: prepare external identities away from the async worker. The stamps are
        // reused for repeated resources and any late bound-resource restamp this run.
        self.resource_stamps
            .prepare_run(
                &self.compiled.program,
                &self.plan,
                &self.cache,
                cancel.clone(),
            )
            .await;

        // Phase 2b: cache-aware refinement. Stamp digests, then derive disposition,
        // exact output demand, and live readers together. The resolved run is authoritative:
        // a cache-hit or blocked consumer contributes no upstream demand.
        self.resolver.resolve(
            &self.compiled.program,
            &self.plan,
            &mut self.cache,
            &self.resource_stamps,
        );

        // Phase 3: run the surviving schedule. Each node's disk cache is written the moment it
        // finishes (inside the run loop), not batched here — so a long run's earlier
        // caches are durable even if a later node fails or the run is cancelled.
        let mut stats = self
            .executor
            .run(
                &self.compiled.program,
                &self.plan,
                &self.resolver.run,
                &mut self.cache,
                &mut self.resource_stamps,
                &self.compiled.flatten_map,
                events,
                cancel,
            )
            .await;

        // Phase 3b: sweep values outside the active RAM-retention frontier.
        self.cache
            .evict_unused(&self.compiled.program, &self.resolver.run.disposition);

        // The resident set is now final (post-eviction), so this is the true
        // cache footprint the run leaves behind — total and per-node.
        let ram = self.cache.resident_ram_stats();
        stats.cache_ram = ram.total;
        stats.node_ram = ram.by_node;

        stats.triggered_events = seeds.events;

        Ok(stats)
    }

    /// Persist to disk any **disk-backed** (`persists_to_disk`, i.e. `Disk`/`Both`)
    /// node that holds a resident value but isn't on disk yet — e.g. a node just toggled to
    /// a disk-backed [`CacheMode`](crate::graph::CacheMode) whose value is still in RAM from
    /// a prior run. The worker calls this on `SaveCaches`, since such a node is a cache hit
    /// and so never re-executes to store itself.
    ///
    /// Never rewrites identical content: a blob already stamped with the node's current
    /// digest is the same bytes, so [`DiskStore::store`] skips it. Also a no-op for a
    /// node with no resident value.
    pub(crate) async fn store_resident_caches(&mut self) {
        for node_id in self.compiled.program.e_nodes.keys().copied() {
            if !self.compiled.program.e_nodes[&node_id]
                .cache
                .persists_to_disk()
            {
                continue;
            }
            self.cache
                .store_node(
                    &self.compiled.program,
                    node_id,
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
    /// Compile + install in one step — the pre-split `update` shape the
    /// in-tree tests are written against. Production compiles on the host
    /// (a long-lived [`compile::Compiler`]) and sends the artifact to the worker.
    pub(crate) fn update(
        &mut self,
        graph: &crate::graph::Graph,
        library: &crate::library::Library,
    ) -> std::result::Result<(), compile::CompileError> {
        self.install(
            compile::Compiler::default()
                .compile(graph, library)?
                .compiled,
        );
        Ok(())
    }

    pub(crate) async fn execute_sinks(&mut self) -> Result<ExecutionStats> {
        self.execute(
            RunSeeds {
                sinks: true,
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

    pub(crate) async fn execute_nodes<T: IntoIterator<Item = NodeAddress>>(
        &mut self,
        nodes: T,
    ) -> Result<ExecutionStats> {
        self.execute(
            RunSeeds {
                nodes: nodes.into_iter().collect(),
                ..Default::default()
            },
            None,
            CancelToken::never(),
        )
        .await
    }

    /// Prepare the structural plan and cache-aware resolved run without invoking lambdas.
    pub(crate) fn prepare_execution(
        &mut self,
        sinks: bool,
        event_triggers: bool,
        events: &[EventRef],
    ) -> Result<()> {
        let seeds = RunSeeds {
            sinks,
            event_triggers,
            events: events.to_vec(),
            nodes: Vec::new(),
        };
        self.planner.plan(&self.compiled, &seeds, &mut self.plan)?;
        self.resource_stamps = RunResourceStamps::default();
        self.resolver.resolve(
            &self.compiled.program,
            &self.plan,
            &mut self.cache,
            &self.resource_stamps,
        );
        Ok(())
    }

    pub(crate) fn node_inputs(&self, node_id: NodeId) -> &[program::ExecutionInput] {
        self.compiled
            .program
            .node_inputs(&self.compiled.program.e_nodes[&node_id])
    }

    pub(crate) fn node_events(&self, node_id: NodeId) -> &[program::ExecutionEvent] {
        let events = self.compiled.program.e_nodes[&node_id].events;
        &self.compiled.program.events[events.range()]
    }

    pub(crate) fn node_output_demand(&self, node_id: NodeId) -> &[OutputDemand] {
        self.resolver
            .run
            .outputs
            .demand
            .slice(self.compiled.program.e_nodes[&node_id].outputs)
    }

    pub(crate) fn node_output_readers(&self, node_id: NodeId) -> &[u32] {
        self.resolver
            .run
            .outputs
            .readers
            .slice(self.compiled.program.e_nodes[&node_id].outputs)
    }

    /// Whether `node_id` recomputed (rather than reused a cache) in the last run.
    pub(crate) fn node_ran(&self, node_id: NodeId) -> bool {
        self.executor.ran(node_id)
    }

    /// Seed a node's cached output (simulating a prior run): set the value and
    /// stamp `produced_under` from the current digest, so the planner sees a hit.
    pub(crate) fn set_output_values(&mut self, node_id: NodeId, values: Vec<DynamicValue>) {
        let slot = self.cache.slots.get_mut(&node_id).unwrap();
        slot.value = cache::ValueState::Resident {
            snapshot: cache::OutputSnapshot::new(values),
            produced_under: slot.current_digest,
        };
    }
}
