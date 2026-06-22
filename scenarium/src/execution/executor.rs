//! Mutable execution-time state and the run loop. The `Executor` owns the
//! per-node runtime `slots` (cache + per-run results), the shared
//! `ctx_manager`, and the invoke scratch. Given an immutable
//! [`ExecutionProgram`](crate::execution::program::ExecutionProgram) and a prepared
//! [`ExecutionPlan`](crate::execution::plan::ExecutionPlan), [`Executor::run`] invokes
//! each scheduled node's lambda and gathers stats.

use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use common::{KeyIndexKey, KeyIndexVec};

use crate::common::shared_any_state::SharedAnyState;
use crate::context::ContextManager;
use crate::data::DynamicValue;
use crate::execution_stats::{
    ExecutedNodeStats, ExecutionStats, FlattenMap, NodeError, RunPhase, RunProgress,
};
use crate::func_lambda::InvokeInput;
use crate::graph::{InputPort, NodeId};
use crate::prelude::AnyState;

use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionNode, ExecutionProgram};
use crate::execution::{Error, OutputUsage};

/// Per-node runtime state, index-aligned to the program's `e_nodes`: the
/// cross-run value cache (`state`, `event_state`, `output_values`) plus per-run
/// results (`error`, `run_time`). Carries its own `id` so the cache can be
/// reconciled by key on `update` (surviving node reorder/trim).
#[derive(Default, Debug)]
pub(crate) struct RuntimeSlot {
    pub(crate) id: NodeId,
    pub(crate) state: AnyState,
    pub(crate) event_state: SharedAnyState,
    pub(crate) output_values: Option<Vec<DynamicValue>>,
    pub(crate) error: Option<Error>,
    pub(crate) run_time: f64,
}

impl KeyIndexKey<NodeId> for RuntimeSlot {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

impl RuntimeSlot {
    fn reset_state(&mut self) {
        self.state = AnyState::default();
        self.event_state = SharedAnyState::default();
        self.output_values = None;
    }
}

#[derive(Default, Debug)]
pub(crate) struct Executor {
    /// Per-node runtime state, index-aligned to `program.e_nodes`.
    pub(crate) slots: KeyIndexVec<NodeId, RuntimeSlot>,
    pub(crate) ctx_manager: ContextManager,
    /// Cross-run per-input dirty bits, index-aligned to `program.inputs`. Set by
    /// `flatten` when a binding changes at `update`, consumed/cleared here when
    /// the owning node runs. Rebuilt positionally by each `flatten` so it tracks
    /// the inputs pool; bridges update→run, unlike the per-run plan flags.
    pub(crate) input_dirty: Vec<bool>,
    /// Per-node invoke scratch, refilled per executed node.
    inputs: Vec<InvokeInput>,
    output_usage: Vec<OutputUsage>,
}

impl Executor {
    /// Rebuild `slots` in `e_nodes` order: preserve each surviving node's cache
    /// by id, default new nodes, trim removed ones. Mirrors the `CompactInsert`
    /// flatten runs over `e_nodes`, keeping `slots[i]` aligned to `e_nodes[i]`.
    pub(crate) fn reconcile(&mut self, e_nodes: &KeyIndexVec<NodeId, ExecutionNode>) {
        let mut compact = self.slots.compact_insert_start();
        for e_node in e_nodes.iter() {
            compact.insert_with(&e_node.id, || RuntimeSlot {
                id: e_node.id,
                ..Default::default()
            });
        }
    }

    pub(crate) fn reset_states(&mut self) {
        for slot in self.slots.iter_mut() {
            slot.reset_state();
        }
    }

    /// Execute every node in `plan.execute_order`, invoking its lambda with the
    /// collected inputs. Mutates the runtime cache and clears each executed
    /// node's per-input dirty bits in `input_dirty`. The `program` is read-only.
    /// Returns per-run stats.
    pub(crate) async fn run(
        &mut self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        flatten: &FlattenMap,
        progress: Option<&UnboundedSender<RunProgress>>,
    ) -> ExecutionStats {
        let start = Instant::now();

        // Clear the previous run's per-run results.
        for slot in self.slots.iter_mut() {
            slot.run_time = 0.0;
            slot.error = None;
        }
        self.ctx_manager.logs.clear();

        let mut inputs = std::mem::take(&mut self.inputs);
        let mut output_usage = std::mem::take(&mut self.output_usage);

        for e_node_idx in plan.execute_order.iter().copied() {
            if program.e_nodes[e_node_idx].lambda.is_none() {
                continue;
            }

            let func_id = program.e_nodes[e_node_idx].func_id;

            if self.has_errored_dependency(program, e_node_idx) {
                let slot = &mut self.slots[e_node_idx];
                slot.output_values = None;
                slot.error = Some(Error::Invoke {
                    func_id,
                    message: "Skipped due to upstream error".to_string(),
                });
                continue;
            }

            self.collect_inputs(program, plan, e_node_idx, &mut inputs);
            self.collect_output_usage(program, plan, e_node_idx, &mut output_usage);

            let output_count = program.e_nodes[e_node_idx].outputs.len as usize;
            let event_state = self.slots[e_node_idx].event_state.clone();
            assert!(self.slots[e_node_idx].error.is_none());

            // Attribute any logs this node emits to it (read by
            // `ContextManager::log`).
            let flat_id = program.e_nodes[e_node_idx].id;
            self.ctx_manager.current_node = Some(flat_id);
            let invoke_start = Instant::now();
            if let Some(progress) = progress {
                let _ = progress.send(RunProgress {
                    nodes: flatten.attribution(flat_id).collect(),
                    phase: RunPhase::Started { at: invoke_start },
                });
            }
            let result = {
                let lambda = &program.e_nodes[e_node_idx].lambda;
                let slot = &mut self.slots[e_node_idx];
                let outputs = slot
                    .output_values
                    .get_or_insert_with(|| vec![DynamicValue::Unbound; output_count]);
                lambda
                    .invoke(
                        &mut self.ctx_manager,
                        &mut slot.state,
                        &event_state,
                        &inputs,
                        &output_usage,
                        outputs,
                    )
                    .await
                    .map_err(|e| Error::Invoke {
                        func_id,
                        message: e.to_string(),
                    })
            };

            let run_time = invoke_start.elapsed().as_secs_f64();
            let slot = &mut self.slots[e_node_idx];
            slot.run_time = run_time;
            if let Err(err) = result {
                slot.error = Some(err);
                slot.output_values = None;
            }
            if let Some(progress) = progress {
                let _ = progress.send(RunProgress {
                    nodes: flatten.attribution(flat_id).collect(),
                    phase: RunPhase::Finished {
                        elapsed_secs: run_time,
                    },
                });
            }

            let span = program.e_nodes[e_node_idx].inputs;
            for dirty in &mut self.input_dirty[span.range()] {
                *dirty = false;
            }
        }

        self.ctx_manager.current_node = None;
        let mut stats = self.collect_execution_stats(program, plan, start);
        stats.logs = std::mem::take(&mut self.ctx_manager.logs);
        self.inputs = inputs;
        self.output_usage = output_usage;
        stats
    }

    fn has_errored_dependency(&self, program: &ExecutionProgram, e_node_idx: usize) -> bool {
        let span = program.e_nodes[e_node_idx].inputs;
        program.inputs[span.range()].iter().any(|input| {
            matches!(&input.binding, ExecutionBinding::Bind(addr) if self.slots[addr.target_idx].error.is_some())
        })
    }

    fn collect_inputs(
        &self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        e_node_idx: usize,
        inputs: &mut Vec<InvokeInput>,
    ) {
        inputs.clear();
        let span = program.e_nodes[e_node_idx].inputs;
        for (i, input) in program.inputs[span.range()].iter().enumerate() {
            let value = match &input.binding {
                ExecutionBinding::None => DynamicValue::Unbound,
                ExecutionBinding::Const(v) => v.into(),
                ExecutionBinding::Bind(addr) => {
                    // Invariant: any node in `execute_order` has every bound
                    // upstream producing output — the planner gates a consumer
                    // (`missing_required_inputs`) whenever a bind target can't
                    // run, *including* optional binds, so a `None` here is a
                    // planner bug, not a graph the user can author.
                    let outputs = self.slots[addr.target_idx]
                        .output_values
                        .as_ref()
                        .expect("missing output values");
                    assert_eq!(
                        outputs.len(),
                        program.e_nodes[addr.target_idx].outputs.len as usize
                    );
                    outputs[addr.port_idx].clone()
                }
            };
            let pool_idx = span.start as usize + i;
            let dependency_wants_execute = plan.input_flags[pool_idx].dependency_wants_execute;
            inputs.push(InvokeInput {
                changed: self.input_dirty[pool_idx] || dependency_wants_execute,
                value,
            });
        }
    }

    fn collect_output_usage(
        &self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        e_node_idx: usize,
        usage: &mut Vec<OutputUsage>,
    ) {
        usage.clear();
        let span = program.e_nodes[e_node_idx].outputs;
        usage.extend(plan.output_usage[span.range()].iter().map(|&c| {
            if c == 0 {
                OutputUsage::Skip
            } else {
                OutputUsage::Needed(c)
            }
        }));
    }

    fn collect_execution_stats(
        &self,
        program: &ExecutionProgram,
        plan: &ExecutionPlan,
        start: Instant,
    ) -> ExecutionStats {
        let mut executed_nodes = Vec::new();
        let mut missing_inputs = Vec::new();
        let mut cached_nodes = Vec::new();
        let mut node_errors = Vec::new();

        for &idx in &plan.execute_order {
            let e = &program.e_nodes[idx];
            executed_nodes.push(ExecutedNodeStats {
                node_id: e.id,
                elapsed_secs: self.slots[idx].run_time,
            });
        }

        for &idx in &plan.process_order {
            let e = &program.e_nodes[idx];
            let flags = plan.node_flags[idx];
            if flags.missing_required_inputs {
                for i in 0..e.inputs.len as usize {
                    if plan.input_flags[e.inputs.start as usize + i].missing {
                        missing_inputs.push(InputPort {
                            node_id: e.id,
                            port_idx: i,
                        });
                    }
                }
            }
            if flags.cached {
                cached_nodes.push(e.id);
            }
            if let Some(err) = &self.slots[idx].error {
                node_errors.push(NodeError {
                    node_id: e.id,
                    error: err.clone(),
                });
            }
        }

        ExecutionStats {
            elapsed_secs: start.elapsed().as_secs_f64(),
            executed_nodes,
            missing_inputs,
            cached_nodes,
            triggered_events: Vec::default(),
            node_errors,
            // Filled by `run` from the context manager's per-run buffer.
            logs: Vec::new(),
            // Filled by `ExecutionEngine::execute` from the flatten pass;
            // the executor doesn't know the authoring graph.
            flatten: FlattenMap::default(),
        }
    }
}
