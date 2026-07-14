//! Node-graph execution as an explicit three-phase pipeline:
//!
//! 1. **compile** — the [`Compiler`](compile::Compiler) flattens the authoring
//!    `Graph` into an immutable [`ExecutionProgram`](program::ExecutionProgram).
//!    Runs on the *host's* thread (compile errors are synchronous); the resulting
//!    [`CompiledGraph`](compile::CompiledGraph) is installed into the engine
//!    via [`ExecutionEngine::install`], which cannot fail.
//! 2. **plan** — the [`Planner`](plan::Planner) turns the program into an
//!    [`ExecutionPlan`](plan::ExecutionPlan) (the schedule). Purely structural —
//!    reachability + topological order + output demand/readers, no cache/digest state.
//! 3. **execute** — the [`Executor`](executor::Executor) first resolves which
//!    pure-structural nodes reuse a cache and cuts every cone that feeds only
//!    reuse hits (so a cached node's stale upstream isn't recomputed on reopen),
//!    then walks the surviving schedule producer-first, computing each node's
//!    content digest and deciding reuse (RAM / disk) or recompute inline.
//!
//! [`ExecutionEngine`] owns the run-side pieces (program, plan, planner, the
//! cross-run cache, and executor) and exposes `install` (phase 1's artifact)
//! and `execute` (phases 2–3, run back-to-back).

use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

use common::{CancelToken, Span};
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
use program::{NodeIdx, OutputIdx};
use resolve::Resolver;

trait ColumnIndex {
    fn idx(self) -> usize;
}

impl ColumnIndex for NodeIdx {
    fn idx(self) -> usize {
        self.idx()
    }
}

impl ColumnIndex for OutputIdx {
    fn idx(self) -> usize {
        self.idx()
    }
}

/// A `Vec<T>` addressable only by one typed program index.
#[derive(Debug, Clone)]
pub(crate) struct Column<I, T> {
    pub(crate) values: Vec<T>,
    index: PhantomData<fn(I)>,
}

impl<I, T> Default for Column<I, T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            index: PhantomData,
        }
    }
}

impl<I, T: Clone> Column<I, T> {
    pub(crate) fn reset(&mut self, len: usize, value: T) {
        self.values.clear();
        self.values.resize(len, value);
    }
}

impl<I, T> From<Vec<T>> for Column<I, T> {
    fn from(values: Vec<T>) -> Self {
        Self {
            values,
            index: PhantomData,
        }
    }
}

impl<I: ColumnIndex, T> Index<I> for Column<I, T> {
    type Output = T;

    fn index(&self, i: I) -> &T {
        &self.values[i.idx()]
    }
}

impl<I: ColumnIndex, T> IndexMut<I> for Column<I, T> {
    fn index_mut(&mut self, i: I) -> &mut T {
        &mut self.values[i.idx()]
    }
}

/// A column aligned to the program's flat node table.
pub(crate) type NodeColumn<T> = Column<NodeIdx, T>;

/// A column aligned to the program's flat output pool. Node-local views are sliced by
/// their compiled output span, while individual entries require an [`OutputIdx`].
pub(crate) type OutputColumn<T> = Column<OutputIdx, T>;

impl<T> Column<OutputIdx, T> {
    pub(crate) fn slice(&self, outputs: Span) -> &[T] {
        &self.values[outputs.range()]
    }

    pub(crate) fn slice_mut(&mut self, outputs: Span) -> &mut [T] {
        &mut self.values[outputs.range()]
    }
}

// === Error Types ===

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
    /// the graph they target, so a miss means inconsistent caller state (or a disabled
    /// target) — the run fails rather than silently skipping the seed.
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
    /// Run the cones of these specific nodes (authoring ids), retaining their outputs
    /// in RAM for read-back — the on-demand "run to this node" / preview trigger. The
    /// worker batches these with the graph they target, so an id that doesn't resolve
    /// against the compiled program (deleted, disabled, stale) fails the run with
    /// [`Error::NodeSeedNotFound`] — inconsistent caller state, never silently skipped.
    pub nodes: Vec<NodeAddress>,
}

// === Execution Engine ===

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
    /// Reusable plan buffer, recycled across runs to avoid reallocation.
    plan: ExecutionPlan,
}

impl ExecutionEngine {
    // === Accessors ===

    pub(crate) fn is_empty(&self) -> bool {
        self.compiled.program.e_nodes.is_empty()
    }

    // === State Management ===

    pub(crate) fn clear(&mut self) {
        self.compiled = CompiledGraph::default();
        self.plan.clear();
        self.cache.clear();
    }

    /// Swap the [`DiskStore`] — the library snapshot (its type table supplies
    /// the custom-value codecs) plus the optional
    /// store root. At the next `install`, `persist` outputs hydrate
    /// from their blobs on a hit (skipping recompute), and freshly-computed ones are
    /// stored after a run. The RAM cache is keyed by node id + digest, independent of the
    /// root, so swapping keeps any warm in-memory outputs.
    pub(crate) fn set_disk_store(&mut self, disk_store: DiskStore) {
        self.cache.disk_store = disk_store;
    }

    // === Phase 1: install the compile artifact ===

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
        self.cache.reconcile(&self.compiled.program.e_nodes);

        self.compiled.validate_installed(&self.cache);
    }

    // === Phases 2–3: plan then execute ===

    /// When `events` is `Some`, a [`RunEvent`] is sent for live per-node
    /// feedback ahead of the final stats: a `RunEvent::Progress` before and
    /// after each node's lambda runs, and a `RunEvent::PinnedOutputs` right
    /// after a node with a pinned output (or that is itself a pinned root)
    /// finishes running, so a GUI preview updates without polling. When
    /// `cancel` is `Some` and gets set mid-run, scheduling stops after the
    /// in-flight node and the returned stats are marked `cancelled`.
    pub(crate) async fn execute(
        &mut self,
        seeds: RunSeeds,
        events: Option<&UnboundedSender<RunEvent>>,
        cancel: CancelToken,
    ) -> Result<ExecutionStats> {
        // Phase 2: schedule into the reusable plan buffer. Purely structural —
        // reachability + topological order + output demand/readers + the walk roots, no cache/digest
        // state. The artifact's flatten map resolves node seeds (authoring ids) to flat roots.
        self.planner.plan(&self.compiled, &seeds, &mut self.plan)?;

        // Phase 2b: cache-aware refinement. Resolve every node's disposition — its reuse
        // verdict merged with the backward cut, so a cone feeding only cache hits (a
        // disk-cached node's stale upstream) isn't recomputed on reopen. Mutates the cache
        // (stamps digests, flags disk hits). The column is authoritative for the run: the
        // executor reads it rather than re-deriving (a digest folds live filesystem state
        // and could drift mid-run).
        self.resolver
            .resolve(&self.compiled.program, &self.plan, &mut self.cache);

        // Phase 3: run the surviving schedule. Each node's disk cache is written the moment it
        // finishes (inside the run loop), not batched here — so a long run's earlier
        // caches are durable even if a later node fails or the run is cancelled.
        let mut stats = self
            .executor
            .run(
                &self.compiled.program,
                &self.plan,
                &self.resolver.disposition,
                &mut self.cache,
                &self.compiled.flatten_map,
                events,
                cancel,
            )
            .await;

        // Phase 3b: reclaim RAM from values this run left off the active frontier and that the
        // disk store (written per-node above) can serve again on demand. Reuses the resolver's
        // disposition column (the active-frontier set) and the executor's retention policy
        // (RAM modes + pinned preview roots) rather than recomputing either.
        self.cache.evict_unused(
            &self.compiled.program,
            &self.resolver.disposition,
            &self.executor.retain,
        );

        // The resident set is now final (post-eviction), so this is the true
        // cache footprint the run leaves behind — total and per-node.
        stats.cache_ram = self.cache.resident_ram_usage();
        stats.node_ram = self.cache.resident_ram_by_node();

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
        for idx in self.compiled.program.node_indices() {
            if !self.compiled.program.e_nodes[idx].cache.persists_to_disk() {
                continue;
            }
            self.cache
                .store_node(&self.compiled.program, idx, &mut self.executor.ctx_manager)
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
        self.install(compile::Compiler::default().compile(graph, library)?);
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

    /// Run only the planning phase (no execution), leaving the schedule in
    /// `self.plan` for inspection.
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
        self.planner.plan(&self.compiled, &seeds, &mut self.plan)
    }

    pub(crate) fn by_id(&self, node_id: &NodeId) -> Option<&ExecutionNode> {
        self.compiled.program.e_nodes.by_key(node_id)
    }

    pub(crate) fn by_name(&self, node_name: &str) -> Option<&ExecutionNode> {
        self.compiled
            .program
            .e_nodes
            .iter()
            .find(|node| node.name == node_name)
    }

    pub(crate) fn node_inputs(&self, e_node: &ExecutionNode) -> &[program::ExecutionInput] {
        self.compiled.program.node_inputs(e_node)
    }

    pub(crate) fn node_events(&self, e_node: &ExecutionNode) -> &[program::ExecutionEvent] {
        &self.compiled.program.events[e_node.events.range()]
    }

    pub(crate) fn node_verdict(&self, e_node: &ExecutionNode) -> plan::NodeVerdict {
        let idx = self
            .compiled
            .program
            .e_nodes
            .index_of_key(&e_node.id)
            .unwrap();
        self.plan.verdicts[idx.into()]
    }

    pub(crate) fn node_output_demand(
        &self,
        e_node: &ExecutionNode,
    ) -> &[crate::node::lambda::OutputDemand] {
        self.plan.outputs.demand.slice(e_node.outputs)
    }

    pub(crate) fn node_output_readers(&self, e_node: &ExecutionNode) -> &[u32] {
        self.plan.outputs.readers.slice(e_node.outputs)
    }

    /// Whether node `idx` recomputed (rather than reused a cache) in the last run.
    pub(crate) fn node_ran(&self, idx: program::NodeIdx) -> bool {
        self.executor.ran(idx)
    }

    pub(crate) fn runtime_slot(&self, e_node: &ExecutionNode) -> &cache::RuntimeSlot {
        self.cache.slots.by_key(&e_node.id).unwrap()
    }

    /// Seed a node's cached output (simulating a prior run): set the value and
    /// stamp `produced_under` from the current digest, so the planner sees a hit.
    pub(crate) fn set_output_values(&mut self, node_name: &str, values: Vec<DynamicValue>) {
        let idx = self
            .compiled
            .program
            .e_nodes
            .index_of_key(&self.by_name(node_name).unwrap().id);
        let idx = idx.unwrap();
        let slot = &mut self.cache.slots[idx];
        let coverage = cache::CachedOutputCoverage::from_values(&values);
        slot.value = cache::ValueState::Resident {
            snapshot: cache::OutputSnapshot::new(values, coverage),
            produced_under: slot.current_digest,
        };
    }
}
