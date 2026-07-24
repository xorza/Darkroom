//! [`ExecutionEngine`] owns the run-side pieces (program, plan, planner, the
//! cross-run cache, and executor) and exposes `install` (phase 1's artifact)
//! and `execute` (phases 2–3, run back-to-back).

use std::sync::Arc;

use common::CancelToken;
use tokio::sync::mpsc::UnboundedSender;

use crate::execution::cache::runtime::{CacheEvictionFailure, RuntimeCache};
use crate::execution::compile::CompiledGraph;
use crate::execution::disk_store::StorePolicy;
use crate::execution::error::Result;
#[cfg(test)]
use crate::execution::error::{Error, RunError};
use crate::execution::executor::Executor;
#[cfg(test)]
use crate::execution::identity::ExecutionEventPort;
use crate::execution::outcome::ExecutionOutcome;
use crate::execution::plan::{ExecutionPlan, Planner};
#[cfg(test)]
use crate::execution::program::ExecutionNode;
use crate::execution::report::RunEvent;
use crate::execution::resolve::Resolver;
use crate::execution::resource::RunResourceStamps;
use crate::execution::seeds::RunSeeds;
use crate::graph::NodeId;
#[cfg(test)]
use crate::node::definition::FuncId;

#[cfg(test)]
mod tests;

/// The run-side pipeline container. Shares the installed program and its
/// execution-attribution map, the reusable `plan` buffer, the `planner`
/// (scheduling scratch), the cross-run `cache` (per-node outputs + state, plus its
/// owned [`disk_store::DiskStore`] file persistence and the caching policy), and the `executor`
/// (run loop + context). Compilation happens on the host ([`compile::Compiler`]);
/// the engine only ever receives ready [`CompiledGraph`]s. Not serializable — the
/// persistent form is the [`ExecutionProgram`] alone.
#[derive(Debug, Default)]
pub(crate) struct ExecutionEngine {
    /// The installed shared compile artifact: the program plus its compact
    /// execution-to-authoring attribution map.
    /// Replaced wholesale by [`Self::install`].
    pub(crate) compiled: Arc<CompiledGraph>,
    /// Per-node cross-run cache (output values, digests, node state) plus the
    /// [`disk_store::DiskStore`]
    /// backing it and the caching policy over both — reuse, hydration, persistence, eviction.
    /// The RAM slots are reconciled to the node set at each `install`; the disk store is set
    /// by the worker and kept across installs.
    pub(crate) cache: RuntimeCache,
    executor: Executor,
    planner: Planner,
    /// Cache-aware refinement of the plan: resolves reuse + cuts cones feeding only cache
    /// hits, between plan and execute. Owns reusable per-run scratch (see `resolve.rs`).
    resolver: Resolver,
    /// Per-run filesystem identities, collected off-thread and shared by initial
    /// resolution and late bound-path restamps.
    resource_stamps: RunResourceStamps,
    /// Reusable plan buffer, recycled across runs to avoid reallocation.
    plan: ExecutionPlan,
}

impl ExecutionEngine {
    pub(crate) fn is_empty(&self) -> bool {
        self.compiled.program.e_nodes.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.compiled = Arc::default();
        self.plan.reset_for_program(&self.compiled.program);
        self.cache.clear();
        self.resource_stamps = RunResourceStamps::default();
    }

    /// Install a host-compiled [`CompiledGraph`] as the current program.
    /// Infallible: everything that can go wrong went wrong at compile
    /// ([`compile::Compiler`]), on the host's thread.
    ///
    /// The plan isn't cleared here: every `execute` re-`plan`s from scratch and nothing
    /// reads the reusable plan buffer between an install and the next run.
    pub(crate) fn install(&mut self, compiled: Arc<CompiledGraph>) {
        self.compiled = compiled;

        // Realign the runtime cache to the new node set (preserve by id,
        // default new, trim gone).
        self.cache.reconcile(&self.compiled.program);

        self.compiled.validate_installed_debug(&self.cache);
    }

    pub(crate) async fn evict_cache(&mut self, node_ids: &[NodeId]) -> Vec<CacheEvictionFailure> {
        let e_node_ids = self.compiled.data_consumer_closure(node_ids);
        self.cache.evict(&e_node_ids).await
    }

    /// When `events` is `Some`, a [`RunEvent`] is sent for live per-node
    /// feedback ahead of the final outcome: a `RunEvent::Progress` before and
    /// after each node's lambda runs, and a `RunEvent::PinnedOutputs` when a
    /// node with a pinned output (or that is itself a pinned root) produces or
    /// reuses its value, so a GUI preview updates without polling. When `cancel`
    /// is set mid-run, scheduling stops after the in-flight node and the
    /// caller-owned outcome is marked `cancelled`. The outcome also owns triggers
    /// initialized successfully by an `event_sources` seed.
    pub(crate) async fn execute(
        &mut self,
        mut seeds: RunSeeds,
        events: Option<&UnboundedSender<RunEvent>>,
        cancel: CancelToken,
        outcome: &mut ExecutionOutcome,
    ) -> Result<()> {
        outcome.clear();

        // Phase 2: schedule into the reusable plan buffer. Purely structural —
        // reachability + topological order + missing-input verdicts + walk roots, no
        // cache/digest state. Node seeds already identify exact compiled roots.
        self.planner.plan(&self.compiled, &seeds, &mut self.plan)?;

        // Phase 2a: prepare filesystem identities away from the async worker. The stamps are
        // reused for repeated paths and any late bound-path restamp this run.
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
        self.resolver
            .resolve(
                &self.compiled.program,
                &self.plan,
                &mut self.cache,
                &self.resource_stamps,
            )
            .await;

        // Phase 3: run the surviving schedule. Each node's disk cache is written the moment it
        // finishes (inside the run loop), not batched here — so a long run's earlier
        // caches are durable even if a later node fails or the run is cancelled.
        self.executor
            .run(
                &self.compiled.program,
                &self.plan,
                &self.resolver.run,
                &mut self.cache,
                &mut self.resource_stamps,
                events,
                cancel,
                outcome,
            )
            .await;

        self.cache.release_dead_outputs(&self.compiled.program);

        // The resident set is now final (post-eviction), so this is the true
        // cache footprint the run leaves behind — total and per-node.
        outcome.cache_ram = self.cache.resident_ram_stats(&mut outcome.node_ram);

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
        for e_node_id in self.compiled.program.e_nodes.keys().copied() {
            if !self.compiled.program.e_nodes[&e_node_id]
                .cache
                .persists_to_disk()
            {
                continue;
            }
            self.cache
                .store_node(
                    &self.compiled.program,
                    e_node_id,
                    StorePolicy::PreserveCovering,
                    &mut self.executor.ctx_manager,
                )
                .await;
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use common::CancelToken;

    use crate::DynamicValue;
    use crate::execution::cache::slot::{OutputSnapshot, ValueState};
    use crate::execution::compile;
    use crate::execution::engine::ExecutionEngine;
    use crate::execution::error::Result;
    use crate::execution::identity::ExecutionEventPort;
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::outcome::ExecutionOutcome;
    use crate::execution::program;
    use crate::execution::program::ExecutionBinding;
    use crate::execution::resource::RunResourceStamps;
    use crate::execution::seeds::RunSeeds;
    use crate::graph::NodeId;
    use crate::node::lambda::OutputDemand;

    #[derive(Debug, Default)]
    pub(crate) struct ArgumentValues {
        pub(crate) inputs: Vec<Option<DynamicValue>>,
        pub(crate) outputs: Vec<DynamicValue>,
    }

    /// Test-only inspection of the last plan's per-run flags and runtime slots.
    impl ExecutionEngine {
        /// Compile + install in one step — the pre-split `update` shape the
        /// in-tree tests are written against. Production compiles on the host
        /// (a long-lived [`compile::Compiler`]) and sends the artifact to the worker.
        pub(crate) fn update(
            &mut self,
            graph: &crate::graph::Graph,
            library: &crate::library::Library,
        ) -> std::result::Result<(), compile::CompileError> {
            self.install(compile::Compiler::default().compile(graph, library)?.into());
            Ok(())
        }

        pub(crate) async fn execute_sinks(&mut self) -> Result<ExecutionOutcome> {
            let mut outcome = ExecutionOutcome::default();
            self.execute(
                RunSeeds {
                    sinks: true,
                    ..Default::default()
                },
                None,
                CancelToken::never(),
                &mut outcome,
            )
            .await?;
            Ok(outcome)
        }

        pub(crate) async fn execute_events<T: IntoIterator<Item = ExecutionEventPort>>(
            &mut self,
            events: T,
        ) -> Result<ExecutionOutcome> {
            let mut outcome = ExecutionOutcome::default();
            self.execute(
                RunSeeds {
                    events: events.into_iter().collect(),
                    ..Default::default()
                },
                None,
                CancelToken::never(),
                &mut outcome,
            )
            .await?;
            Ok(outcome)
        }

        pub(crate) async fn execute_nodes<T: IntoIterator<Item = ExecutionNodeId>>(
            &mut self,
            nodes: T,
        ) -> Result<ExecutionOutcome> {
            let mut outcome = ExecutionOutcome::default();
            self.execute(
                RunSeeds {
                    nodes: nodes.into_iter().collect(),
                    ..Default::default()
                },
                None,
                CancelToken::never(),
                &mut outcome,
            )
            .await?;
            Ok(outcome)
        }

        /// Prepare the structural plan and cache-aware resolved run without invoking lambdas.
        pub(crate) async fn prepare_execution(
            &mut self,
            sinks: bool,
            event_sources: bool,
            events: &[ExecutionEventPort],
        ) -> Result<()> {
            let seeds = RunSeeds {
                sinks,
                event_sources,
                events: events.to_vec(),
                nodes: Vec::new(),
            };
            self.planner.plan(&self.compiled, &seeds, &mut self.plan)?;
            self.resource_stamps = RunResourceStamps::default();
            self.resolver
                .resolve(
                    &self.compiled.program,
                    &self.plan,
                    &mut self.cache,
                    &self.resource_stamps,
                )
                .await;
            Ok(())
        }

        pub(crate) fn node_inputs(&self, e_node_id: ExecutionNodeId) -> &[program::ExecutionInput] {
            let program = &self.compiled.program;
            &program.inputs[program.e_nodes[&e_node_id].inputs]
        }

        pub(crate) fn node_events(&self, e_node_id: ExecutionNodeId) -> &[program::ExecutionEvent] {
            let events = self.compiled.program.e_nodes[&e_node_id].events;
            &self.compiled.program.events[events]
        }

        pub(crate) fn node_output_demand(&self, e_node_id: ExecutionNodeId) -> &[OutputDemand] {
            self.resolver
                .run
                .outputs
                .demand
                .slice(self.compiled.program.e_nodes[&e_node_id].outputs)
        }

        pub(crate) fn node_output_readers(&self, e_node_id: ExecutionNodeId) -> &[u32] {
            self.resolver
                .run
                .outputs
                .readers
                .slice(self.compiled.program.e_nodes[&e_node_id].outputs)
        }

        /// Whether `e_node_id` recomputed (rather than reused a cache) in the last run.
        pub(crate) fn node_ran(&self, e_node_id: ExecutionNodeId) -> bool {
            self.executor.ran(e_node_id)
        }

        /// Resident-only argument values, test inspection only: reads whatever is
        /// in RAM, so a disk-only (not-yet-hydrated) node reads back empty.
        pub(crate) fn get_argument_values(&self, node_id: &NodeId) -> Option<ArgumentValues> {
            self.get_argument_values_at(ExecutionNodeId::from_authoring(&[*node_id]))
        }

        pub(crate) fn get_argument_values_at(
            &self,
            e_node_id: ExecutionNodeId,
        ) -> Option<ArgumentValues> {
            self.compiled.program.e_nodes.get(&e_node_id)?;
            Some(self.argument_values_at(e_node_id))
        }

        fn argument_values_at(&self, e_node_id: ExecutionNodeId) -> ArgumentValues {
            let e_node = &self.compiled.program.e_nodes[&e_node_id];

            let inputs = self.compiled.program.inputs[e_node.inputs]
                .iter()
                .map(|input| match &input.binding {
                    ExecutionBinding::None => None,
                    ExecutionBinding::Const(value) => Some(DynamicValue::from(value)),
                    ExecutionBinding::Bind(address) => self.cache.slots[&address.e_node_id]
                        .output_values()
                        .and_then(|outputs| outputs.get(address.port_idx))
                        .cloned(),
                })
                .collect();

            let outputs = self.cache.slots[&e_node_id]
                .output_values()
                .map(|outputs| outputs.to_vec())
                .unwrap_or_default();

            ArgumentValues { inputs, outputs }
        }

        /// Seed a node's cached output (simulating a prior run): set the value and
        /// stamp `produced_under` from the current digest, so the planner sees a hit.
        pub(crate) fn set_output_values(
            &mut self,
            e_node_id: ExecutionNodeId,
            values: Vec<DynamicValue>,
        ) {
            let slot = self.cache.slots.get_mut(&e_node_id).unwrap();
            slot.value = ValueState::Resident {
                snapshot: OutputSnapshot::new(values),
                produced_under: slot.current_digest,
            };
        }
    }
}
