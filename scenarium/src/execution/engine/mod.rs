//! [`ExecutionEngine`] owns the run-side pieces (program, the run schedule, planner,
//! the cross-run cache, and executor) and exposes `install` (phase 1's artifact)
//! and `execute` (phases 2–3, run back-to-back).

mod error;

use std::sync::Arc;

use ::common::{CancelToken, is_debug};

use crate::RamUsage;
use crate::common::column::Column;
use crate::execution::cache::disk_store::{DiskStore, StorePolicy};
use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::runtime::error::CacheEvictionFailure;
use crate::execution::compiled::CompiledGraph;
use crate::execution::engine::error::InstallValidationError;
use crate::execution::error::Result;
use crate::execution::executor::{Executor, RunRequest};
use crate::execution::identity::NodeIdx;
use crate::execution::report::ExecutionOutcome;
use crate::execution::report::RunReporter;
use crate::execution::schedule::RunSchedule;
use crate::execution::schedule::planner::Planner;
use crate::execution::seeds::RunSeeds;
use crate::graph::identity::NodeId;

/// The run-side pipeline container. Shares the installed program and its
/// execution-attribution map, the reusable `schedule` buffer, the `planner`
/// (scheduling scratch), the cross-run `cache` (per-node outputs + state, plus its
/// owned [`DiskStore`](crate::execution::cache::disk_store::DiskStore) file persistence and the caching policy), and the `executor`
/// (run loop + context). Compilation happens on the host ([`Compiler`](crate::execution::compile::Compiler));
/// the engine only ever receives ready [`CompiledGraph`]s. Not serializable — the
/// persistent form is the [`CompiledGraph`](crate::execution::compiled::CompiledGraph) alone.
#[derive(Debug, Default)]
pub(crate) struct ExecutionEngine {
    /// The installed immutable artifact. Replaced only by [`Self::install`],
    /// which reconciles `cache` onto it in the same step, so the two never move
    /// independently.
    compiled: Option<Arc<CompiledGraph>>,
    /// The cross-run cache, its slots index-aligned to `compiled`'s dense node
    /// space. The cache holds no artifact handle of its own — a program reaches
    /// its methods as an argument, from whoever is reading the pair — so "the
    /// slots and the program are the same install" is established by
    /// [`Self::install`] being the one place the two ever move, rather than
    /// asserted afterwards by comparing two handles. [`Self::validate`] states
    /// the invariants of the pair.
    cache: RuntimeCache,
    executor: Executor,
    planner: Planner,
    /// The one per-run state buffer: the schedule the planner builds and the
    /// dispositions, demand, and reader counts the cache-aware sweep refines it into.
    /// Recycled across runs to avoid reallocation.
    schedule: RunSchedule,
    /// What each node's cache holds once a run has released everything dead — filled by
    /// the cache, read by the executor when it reduces the run to status rows. It lives
    /// here because only the engine sees both ends of that handoff.
    node_ram: Column<NodeIdx, RamUsage>,
}

impl ExecutionEngine {
    pub(crate) fn is_empty(&self) -> bool {
        self.compiled
            .as_deref()
            .is_none_or(|compiled| compiled.e_nodes.is_empty())
    }

    pub(crate) fn clear(&mut self) {
        self.compiled = None;
        self.cache.clear();
        self.schedule = RunSchedule::default();
    }

    /// Install a host-compiled [`CompiledGraph`] as the current program, replacing the
    /// immutable artifact and reconciling the cache onto its dense node space as one
    /// operation. Infallible: everything that can go wrong went wrong at compile
    /// ([`Compiler`](crate::execution::compile::Compiler)), on the host's thread.
    ///
    /// Both programs are in hand here — the one the slots currently belong to and the
    /// one they are moving onto — which is why `reconcile` is told the outgoing one
    /// rather than remembering it.
    ///
    /// The schedule isn't cleared here: every `execute` re-`plan`s from scratch and nothing
    /// reads the reusable buffer between an install and the next run.
    pub(crate) fn install(&mut self, compiled: Arc<CompiledGraph>) {
        let previous = self.compiled.as_deref();
        self.cache.reconcile(previous, &compiled);
        self.compiled = Some(compiled);
        self.validate_debug();
    }

    pub(crate) fn set_disk_store(&mut self, disk_store: DiskStore) {
        self.cache.set_disk_store(disk_store);
    }

    pub(crate) async fn evict_cache(&mut self, node_ids: &[NodeId]) -> Vec<CacheEvictionFailure> {
        let Some(compiled) = self.compiled.as_deref() else {
            return Vec::new();
        };
        self.cache.evict(compiled, node_ids).await
    }

    /// `reporter` receives live feedback ahead of the final outcome: progress before and
    /// after each node's lambda runs, and the pinned outputs of a node that produces or
    /// reuses one (or is itself a pinned root), so a GUI preview updates without polling.
    /// When `cancel` is set mid-run, scheduling stops after the in-flight node and the
    /// caller-owned outcome is marked `cancelled`. The outcome also owns triggers
    /// initialized successfully by an `event_sources` seed.
    pub(crate) async fn execute(
        &mut self,
        mut seeds: RunSeeds,
        reporter: &mut dyn RunReporter,
        cancel: CancelToken,
        outcome: &mut ExecutionOutcome,
    ) -> Result<()> {
        outcome.clear();

        // Phase 2: schedule into the reusable buffer. Purely structural —
        // reachability + topological order + missing-input verdicts + walk roots, no
        // cache/digest state. Node seeds already identify exact compiled roots.
        //
        // Each phase below consumes the previous one's handle, so the order these three
        // run in is the only order that type-checks.
        let compiled = self
            .compiled
            .as_deref()
            .expect("execution requires an installed compiled graph");
        let scheduled = self.planner.plan(compiled, &seeds, &mut self.schedule)?;

        // Phase 2a: prepare filesystem identities away from the async worker. The stamps are
        // reused for repeated paths and any late bound-path restamp this run.
        self.cache
            .prepare(compiled, scheduled.executing(), cancel.clone())
            .await;

        // Phase 2b: cache-aware refinement, into the same buffer. Stamp digests, then derive
        // disposition, exact output demand, and live readers together. The resolved run is
        // authoritative: a cache-hit or blocked consumer contributes no upstream demand.
        let resolved = scheduled.resolve(&mut self.cache).await;

        // Phase 3: run the surviving schedule. Each node's disk cache is written the moment it
        // finishes (inside the run loop), not batched here — so a long run's earlier
        // caches are durable even if a later node fails or the run is cancelled.
        self.executor
            .run(
                RunRequest {
                    run: resolved,
                    cache: &mut self.cache,
                    reporter,
                    cancel,
                },
                outcome,
            )
            .await;

        self.cache.release_dead_outputs(resolved.program());

        // The resident set is now final (post-eviction), so this is the true
        // cache footprint the run leaves behind — total and per-node.
        outcome.cache_ram = self.cache.resident_ram_stats(&mut self.node_ram);

        // Phase 4: reduce the run to one status row per node. Last, because a node's row
        // carries the RAM it ended up holding — which the two steps above just settled —
        // alongside what it did. `resolved` still names the pair the loop walked, so the
        // reduction cannot be taken against a different program or schedule.
        self.executor
            .collect_outcome(resolved, &self.node_ram, outcome);

        outcome.triggered_events.append(&mut seeds.events);

        Ok(())
    }

    /// Persist any resident **disk-backed** (`persists_to_disk`, i.e. `Disk`/`Both`)
    /// values when the worker attaches a new
    /// [`DiskStore`](crate::execution::cache::disk_store::DiskStore). This makes values computed
    /// while the store was memory-only durable once a document receives a cache root.
    ///
    /// The attached store has no reuse verdict for these values, so each current resident
    /// snapshot preserves an existing blob that already covers it. Also a no-op for a node with
    /// no resident value.
    pub(crate) async fn store_resident_caches(&mut self) {
        let Some(compiled) = self.compiled.as_deref() else {
            return;
        };
        for node_idx in (0..compiled.e_nodes.len()).map(|i| NodeIdx(i as u32)) {
            if !compiled[node_idx].cache.persists_to_disk() {
                continue;
            }
            self.cache
                .store_node(
                    compiled,
                    node_idx,
                    StorePolicy::PreserveCovering,
                    &mut self.executor.ctx_manager.contexts,
                )
                .await;
        }
    }

    /// Self-consistency of the installed artifact and the cache aligned to it.
    fn validate(&self) -> std::result::Result<(), InstallValidationError> {
        let program = &self
            .compiled
            .as_deref()
            .expect("validation requires an installed compiled graph");
        if self.cache.slot_count() != program.e_nodes.len() {
            return Err(InstallValidationError::NodeCount {
                slots: self.cache.slot_count(),
                expected: program.e_nodes.len(),
            });
        }

        for ((node_id, e_node), slot) in program
            .node_ids
            .iter()
            .zip(program.e_nodes.iter())
            .zip(self.cache.slots())
        {
            if let Some(output_values) = slot.output_values()
                && output_values.len() != e_node.outputs.len as usize
            {
                return Err(InstallValidationError::OutputArity { node_id: *node_id });
            }
            if slot.owner != e_node.func_id {
                return Err(InstallValidationError::StateOwner { node_id: *node_id });
            }
        }
        Ok(())
    }

    fn validate_debug(&self) {
        if !is_debug() {
            return;
        }
        self.validate()
            .expect("installed compiled graph invariant violated");
    }
}

#[cfg(test)]
mod internals {
    use crate::execution::identity::NodeIdx;
    use ::common::CancelToken;

    use crate::DynamicValue;
    use crate::execution::cache::slot::{OutputSnapshot, RuntimeSlot};
    use crate::execution::compile;
    use crate::execution::compiled::ExecutionBinding;
    use crate::execution::compiled::{CompiledGraph, ExecutionInput};
    use crate::execution::engine::ExecutionEngine;
    use crate::execution::error::Result;
    use crate::execution::report::ExecutionOutcome;
    use crate::execution::report::internals::DiscardedReports;
    use crate::execution::schedule::NodeState;
    use crate::execution::seeds::RunSeeds;
    use crate::graph::Graph;
    use crate::graph::func::lambda::OutputDemand;
    use crate::graph::identity::{EventPort, NodeId};
    use crate::library::Library;

    #[derive(Debug, Default)]
    pub(super) struct ArgumentValues {
        pub(super) inputs: Vec<Option<DynamicValue>>,
        pub(super) outputs: Vec<DynamicValue>,
    }

    /// Test-only inspection of the last plan's per-run flags and runtime slots.
    impl ExecutionEngine {
        /// The installed artifact itself, for test-only introspection.
        /// Production reaches the artifact and its cache through the methods
        /// above, which is what keeps the two moving together.
        pub(super) fn compiled(&self) -> &CompiledGraph {
            self.compiled
                .as_deref()
                .expect("execution requires an installed compiled graph")
        }

        /// Compile + install in one step — the pre-split `update` shape the
        /// in-tree tests are written against. Production compiles on the host
        /// (a long-lived [`compile::Compiler`]) and sends the artifact to the worker.
        pub(super) fn update(
            &mut self,
            graph: &Graph,
            library: &Library,
        ) -> std::result::Result<(), compile::error::CompileError> {
            self.install(compile::Compiler::default().compile(graph, library)?.into());
            Ok(())
        }

        pub(super) async fn execute_sinks(&mut self) -> Result<ExecutionOutcome> {
            let mut outcome = ExecutionOutcome::default();
            self.execute(
                RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                &mut DiscardedReports,
                CancelToken::never(),
                &mut outcome,
            )
            .await?;
            Ok(outcome)
        }

        pub(super) async fn execute_events<T: IntoIterator<Item = EventPort>>(
            &mut self,
            events: T,
        ) -> Result<ExecutionOutcome> {
            let mut outcome = ExecutionOutcome::default();
            self.execute(
                RunSeeds {
                    events: events.into_iter().collect(),
                    ..Default::default()
                },
                &mut DiscardedReports,
                CancelToken::never(),
                &mut outcome,
            )
            .await?;
            Ok(outcome)
        }

        pub(super) async fn execute_nodes<T: IntoIterator<Item = NodeId>>(
            &mut self,
            nodes: T,
        ) -> Result<ExecutionOutcome> {
            let mut outcome = ExecutionOutcome::default();
            self.execute(
                RunSeeds {
                    node_ids: nodes.into_iter().collect(),
                    ..Default::default()
                },
                &mut DiscardedReports,
                CancelToken::never(),
                &mut outcome,
            )
            .await?;
            Ok(outcome)
        }

        /// Prepare the structural plan and cache-aware resolved run without invoking lambdas.
        pub(super) async fn prepare_execution(
            &mut self,
            sinks: bool,
            event_sources: bool,
            events: &[EventPort],
        ) -> Result<()> {
            let seeds = RunSeeds {
                sinks,
                event_sources,
                events: events.to_vec(),
                node_ids: Vec::new(),
            };
            let compiled = self
                .compiled
                .as_deref()
                .expect("execution preparation requires an installed compiled graph");
            self.planner
                .plan(compiled, &seeds, &mut self.schedule)?
                .resolve(&mut self.cache)
                .await;
            Ok(())
        }

        /// Where a stable id sits in the installed program — the one lookup the
        /// introspection below shares, so a test naming a node the install does
        /// not hold fails here rather than indexing something else.
        fn node_idx(&self, node_id: NodeId) -> NodeIdx {
            self.compiled()
                .node(node_id)
                .expect("introspection names a node of the installed program")
        }

        /// The resolved state for a stable id — test introspection.
        pub(super) fn node_state(&self, node_id: NodeId) -> NodeState {
            self.schedule.states[self.node_idx(node_id)]
        }

        pub(super) fn node_inputs(&self, node_id: NodeId) -> &[ExecutionInput] {
            let program = &self.compiled();
            &program.inputs[program.by_id(node_id).inputs]
        }

        pub(super) fn node_output_demand(&self, node_id: NodeId) -> &[OutputDemand] {
            &self.schedule.outputs.demand[self.compiled().by_id(node_id).outputs]
        }

        pub(super) fn node_output_readers(&self, node_id: NodeId) -> &[u32] {
            &self.schedule.outputs.readers[self.compiled().by_id(node_id).outputs]
        }

        /// Whether `node_id` recomputed (rather than reused a cache) in the last run.
        pub(super) fn node_ran(&self, node_id: NodeId) -> bool {
            self.executor.ran(self.compiled(), node_id)
        }

        /// Resident-only argument values, test inspection only: reads whatever is
        /// in RAM, so a disk-only (not-yet-hydrated) node reads back empty.
        pub(super) fn get_argument_values(&self, node_id: &NodeId) -> Option<ArgumentValues> {
            self.get_argument_values_at(*node_id)
        }

        pub(super) fn get_argument_values_at(&self, node_id: NodeId) -> Option<ArgumentValues> {
            self.compiled().node(node_id)?;
            Some(self.argument_values_at(node_id))
        }

        fn argument_values_at(&self, node_id: NodeId) -> ArgumentValues {
            let e_node = &self.compiled().by_id(node_id);

            let inputs = self.compiled().inputs[e_node.inputs]
                .iter()
                .map(|input| match &input.binding {
                    ExecutionBinding::None => None,
                    ExecutionBinding::Const(value) => Some(DynamicValue::from(value)),
                    ExecutionBinding::Bind(address) => self.cache[address.node_idx]
                        .output_values()
                        .and_then(|outputs| outputs.get(address.port_idx as usize))
                        .cloned(),
                })
                .collect();

            let outputs = self.cache[self.node_idx(node_id)]
                .output_values()
                .map(|outputs| outputs.to_vec())
                .unwrap_or_default();

            ArgumentValues { inputs, outputs }
        }

        /// The runtime slot for a stable id — test introspection.
        pub(super) fn slot(&self, node_id: NodeId) -> &RuntimeSlot {
            &self.cache[self.node_idx(node_id)]
        }

        /// Seed a node's cached output (simulating a prior run): set the value and
        /// stamp `produced_under` from the current digest, so the planner sees a hit.
        pub(super) fn set_output_values(&mut self, node_id: NodeId, values: Vec<DynamicValue>) {
            let node_idx = self.node_idx(node_id);
            let slot = &mut self.cache[node_idx];
            let produced_under = slot.current_digest;
            slot.load_output(OutputSnapshot::new(values), produced_under);
        }
    }
}

#[cfg(test)]
mod tests;
