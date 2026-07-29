//! [`ExecutionEngine`] owns the run-side pieces (program, the run schedule, planner,
//! the cross-run cache, and executor) and exposes `install` (phase 1's artifact)
//! and `execute` (phases 2–3, run back-to-back).

use std::sync::Arc;

use common::CancelToken;

use crate::RamUsage;
use crate::execution::cache::disk_store::StorePolicy;
use crate::execution::cache::runtime::{CacheEvictionFailure, RuntimeCache};
use crate::execution::compile::CompiledGraph;
use crate::execution::error::Result;
use crate::execution::executor::{Executor, RunRequest};
use crate::execution::outcome::ExecutionOutcome;
use crate::execution::program::index::{NodeColumn, NodeIdx};
use crate::execution::report::RunReporter;
use crate::execution::schedule::RunSchedule;
use crate::execution::schedule::planner::Planner;
use crate::execution::seeds::RunSeeds;
use crate::graph::address::NodeId;

/// The run-side pipeline container. Shares the installed program and its
/// execution-attribution map, the reusable `schedule` buffer, the `planner`
/// (scheduling scratch), the cross-run `cache` (per-node outputs + state, plus its
/// owned [`disk_store::DiskStore`] file persistence and the caching policy), and the `executor`
/// (run loop + context). Compilation happens on the host ([`compile::Compiler`]);
/// the engine only ever receives ready [`CompiledGraph`]s. Not serializable — the
/// persistent form is the [`Program`] alone.
#[derive(Debug, Default)]
pub(crate) struct ExecutionEngine {
    /// The installed shared compile artifact: the program plus its compact
    /// execution-to-authoring attribution map.
    /// Replaced wholesale by [`Self::install`].
    compiled: Arc<CompiledGraph>,
    /// Per-node cross-run cache (output values, digests, node state) plus the
    /// [`disk_store::DiskStore`]
    /// backing it and the caching policy over both — reuse, hydration, persistence, eviction.
    /// The RAM slots are reconciled to the node set at each `install`; the disk store is set
    /// by the worker and kept across installs.
    pub(crate) cache: RuntimeCache,
    executor: Executor,
    planner: Planner,
    /// The one per-run state buffer: the schedule the planner builds and the
    /// dispositions, demand, and reader counts the cache-aware sweep refines it into.
    /// Recycled across runs to avoid reallocation.
    schedule: RunSchedule,
    /// What each node's cache holds once a run has released everything dead — filled by
    /// the cache, read by the executor when it reduces the run to status rows. It lives
    /// here because only the engine sees both ends of that handoff.
    node_ram: NodeColumn<RamUsage>,
}

impl ExecutionEngine {
    pub(crate) fn is_empty(&self) -> bool {
        self.compiled.program.e_nodes.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.compiled = Arc::default();
        self.schedule.reset_for_program(&self.compiled.program);
        self.cache.clear();
    }

    /// Install a host-compiled [`CompiledGraph`] as the current program.
    /// Infallible: everything that can go wrong went wrong at compile
    /// ([`compile::Compiler`]), on the host's thread.
    ///
    /// The schedule isn't cleared here: every `execute` re-`plan`s from scratch and nothing
    /// reads the reusable buffer between an install and the next run.
    pub(crate) fn install(&mut self, compiled: Arc<CompiledGraph>) {
        // Realign the runtime cache to the new node set (preserve by id,
        // default new, trim gone) — before the swap, while the program its
        // slots are still aligned to is in hand to name them by.
        self.cache
            .reconcile(&self.compiled.program, &compiled.program);
        self.compiled = compiled;

        self.compiled.validate_installed_debug(&self.cache);
    }

    pub(crate) async fn evict_cache(&mut self, node_ids: &[NodeId]) -> Vec<CacheEvictionFailure> {
        let e_node_ids = self.compiled.data_consumer_closure(node_ids);
        self.cache.evict(&self.compiled.program, &e_node_ids).await
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
        let scheduled = self
            .planner
            .plan(&self.compiled.program, &seeds, &mut self.schedule)?;

        // Phase 2a: prepare filesystem identities away from the async worker. The stamps are
        // reused for repeated paths and any late bound-path restamp this run.
        self.cache
            .prepare(scheduled.program(), scheduled.executing(), cancel.clone())
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
        outcome.cache_ram = self
            .cache
            .resident_ram_stats(resolved.program(), &mut self.node_ram);

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
    /// values when the worker attaches a new [`disk_store::DiskStore`]. This makes values computed
    /// while the store was memory-only durable once a document receives a cache root.
    ///
    /// The attached store has no reuse verdict for these values, so each current resident
    /// snapshot preserves an existing blob that already covers it. Also a no-op for a node with
    /// no resident value.
    pub(crate) async fn store_resident_caches(&mut self) {
        for node_idx in (0..self.compiled.program.e_nodes.len()).map(|i| NodeIdx(i as u32)) {
            if !self.compiled.program[node_idx].cache.persists_to_disk() {
                continue;
            }
            self.cache
                .store_node(
                    &self.compiled.program,
                    node_idx,
                    StorePolicy::PreserveCovering,
                    &mut self.executor.ctx_manager.contexts,
                )
                .await;
        }
    }
}

#[cfg(test)]
mod internals {
    use common::CancelToken;

    use crate::DynamicValue;
    use crate::execution::cache::slot::{OutputSnapshot, RuntimeSlot, ValueState};
    use crate::execution::compile;
    use crate::execution::engine::ExecutionEngine;
    use crate::execution::error::Result;
    use crate::execution::identity::ExecutionEventPort;
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::outcome::ExecutionOutcome;
    use crate::execution::program;
    use crate::execution::program::ExecutionBinding;
    use crate::execution::report::internals::DiscardedReports;
    use crate::execution::schedule::NodeState;
    use crate::execution::seeds::RunSeeds;
    use crate::graph::address::NodeId;
    use crate::node::lambda::OutputDemand;

    #[derive(Debug, Default)]
    pub(super) struct ArgumentValues {
        pub(super) inputs: Vec<Option<DynamicValue>>,
        pub(super) outputs: Vec<DynamicValue>,
    }

    /// Test-only inspection of the last plan's per-run flags and runtime slots.
    impl ExecutionEngine {
        /// Compile + install in one step — the pre-split `update` shape the
        /// in-tree tests are written against. Production compiles on the host
        /// (a long-lived [`compile::Compiler`]) and sends the artifact to the worker.
        pub(super) fn update(
            &mut self,
            graph: &crate::graph::Graph,
            library: &crate::library::Library,
        ) -> std::result::Result<(), compile::CompileError> {
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

        pub(super) async fn execute_events<T: IntoIterator<Item = ExecutionEventPort>>(
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

        pub(super) async fn execute_nodes<T: IntoIterator<Item = ExecutionNodeId>>(
            &mut self,
            nodes: T,
        ) -> Result<ExecutionOutcome> {
            let mut outcome = ExecutionOutcome::default();
            self.execute(
                RunSeeds {
                    e_node_ids: nodes.into_iter().collect(),
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
            events: &[ExecutionEventPort],
        ) -> Result<()> {
            let seeds = RunSeeds {
                sinks,
                event_sources,
                events: events.to_vec(),
                e_node_ids: Vec::new(),
            };
            self.planner
                .plan(&self.compiled.program, &seeds, &mut self.schedule)?
                .resolve(&mut self.cache)
                .await;
            Ok(())
        }

        /// The resolved state for a stable id — test introspection.
        pub(super) fn node_state(&self, e_node_id: ExecutionNodeId) -> NodeState {
            self.schedule.states[self.compiled.program.e_node_index[&e_node_id]]
        }

        pub(super) fn node_inputs(&self, e_node_id: ExecutionNodeId) -> &[program::ExecutionInput] {
            let program = &self.compiled.program;
            &program.inputs[program.by_id(e_node_id).inputs]
        }

        pub(super) fn node_events(&self, e_node_id: ExecutionNodeId) -> &[program::ExecutionEvent] {
            let events = self.compiled.program.by_id(e_node_id).events;
            &self.compiled.program.events[events]
        }

        pub(super) fn node_output_demand(&self, e_node_id: ExecutionNodeId) -> &[OutputDemand] {
            self.schedule
                .outputs
                .demand
                .slice(self.compiled.program.by_id(e_node_id).outputs)
        }

        pub(super) fn node_output_readers(&self, e_node_id: ExecutionNodeId) -> &[u32] {
            self.schedule
                .outputs
                .readers
                .slice(self.compiled.program.by_id(e_node_id).outputs)
        }

        /// Whether `e_node_id` recomputed (rather than reused a cache) in the last run.
        pub(super) fn node_ran(&self, e_node_id: ExecutionNodeId) -> bool {
            self.executor.ran(&self.compiled.program, e_node_id)
        }

        /// Resident-only argument values, test inspection only: reads whatever is
        /// in RAM, so a disk-only (not-yet-hydrated) node reads back empty.
        pub(super) fn get_argument_values(&self, node_id: &NodeId) -> Option<ArgumentValues> {
            self.get_argument_values_at(ExecutionNodeId::from_authoring(&[*node_id]))
        }

        pub(super) fn get_argument_values_at(
            &self,
            e_node_id: ExecutionNodeId,
        ) -> Option<ArgumentValues> {
            self.compiled.program.e_node_index.get(&e_node_id)?;
            Some(self.argument_values_at(e_node_id))
        }

        fn argument_values_at(&self, e_node_id: ExecutionNodeId) -> ArgumentValues {
            let e_node = &self.compiled.program.by_id(e_node_id);

            let inputs = self.compiled.program.inputs[e_node.inputs]
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

            let outputs = self.cache[self.compiled.program.e_node_index[&e_node_id]]
                .output_values()
                .map(|outputs| outputs.to_vec())
                .unwrap_or_default();

            ArgumentValues { inputs, outputs }
        }

        /// The runtime slot for a stable id — test introspection.
        pub(super) fn slot(&self, e_node_id: ExecutionNodeId) -> &RuntimeSlot {
            &self.cache[self.compiled.program.e_node_index[&e_node_id]]
        }

        /// Seed a node's cached output (simulating a prior run): set the value and
        /// stamp `produced_under` from the current digest, so the planner sees a hit.
        pub(super) fn set_output_values(
            &mut self,
            e_node_id: ExecutionNodeId,
            values: Vec<DynamicValue>,
        ) {
            let node_idx = self.compiled.program.e_node_index[&e_node_id];
            let slot = &mut self.cache[node_idx];
            slot.value = ValueState::Resident {
                snapshot: OutputSnapshot::new(values),
                produced_under: slot.current_digest,
            };
        }
    }
}

#[cfg(test)]
mod tests;
