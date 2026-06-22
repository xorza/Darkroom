//! Node-graph execution as an explicit three-phase pipeline:
//!
//! 1. **compile** — [`ExecutionEngine::update`] flattens the authoring `Graph`
//!    into an immutable [`ExecutionProgram`](program::ExecutionProgram).
//! 2. **plan** — the [`Planner`](planner::Planner) turns the program + current
//!    cache state into an [`ExecutionPlan`](plan::ExecutionPlan) (the schedule).
//! 3. **execute** — the [`Executor`](executor::Executor) runs the plan,
//!    invoking each scheduled node and updating its runtime cache.
//!
//! [`ExecutionEngine`] owns all four pieces (program, plan, planner, executor)
//! and exposes `update` (phase 1) and `execute*` (phases 2–3, run back-to-back).

use common::CancelToken;
use common::is_debug;
use hashbrown::HashSet;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc::UnboundedSender;

use crate::data::DynamicValue;
use crate::execution_stats::{ExecutionStats, FlattenMap, RunProgress};
use crate::function::FuncLib;
use crate::graph::{Graph, NodeId};
use crate::prelude::FuncId;
use crate::worker::{EventRef, EventTrigger};

pub(crate) mod executor;
mod flatten;
pub(crate) mod plan;
pub(crate) mod planner;
pub(crate) mod program;
#[cfg(test)]
mod tests;

use executor::Executor;
use plan::ExecutionPlan;
use planner::Planner;
use program::{ExecutionBinding, ExecutionNode, ExecutionProgram};

// === Error Types ===

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum Error {
    #[error("{message}")]
    Invoke { func_id: FuncId, message: String },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputUsage {
    Skip,
    /// Number of executing consumers reading this output this run (always `> 0`).
    Needed(u32),
}

impl OutputUsage {
    pub fn is_needed(&self) -> bool {
        matches!(self, Self::Needed(_))
    }
}

// === Execution Engine ===

/// The three-phase pipeline container. Owns the compiled `program`, the
/// `flattener` (compile scratch), the reusable `plan` buffer, the `planner`
/// (scheduling scratch), and the `executor` (runtime cache + context). Not
/// serializable — the persistent form is the [`ExecutionProgram`] alone.
#[derive(Debug, Default)]
pub struct ExecutionEngine {
    pub(crate) program: ExecutionProgram,
    /// Reusable subgraph-flattening scratch (kept across compiles).
    flattener: flatten::Flattener,
    /// How the last `update` flattened the graph (authoring↔execution id
    /// map). Rebuilt each compile, cloned into each run's `ExecutionStats`
    /// so the editor can project stats onto its nodes. Compile scratch,
    /// not part of the serialized program.
    flatten: FlattenMap,
    executor: Executor,
    planner: Planner,
    /// Reusable plan buffer, recycled across runs to avoid reallocation.
    plan: ExecutionPlan,
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
        self.executor.slots.clear();
        self.executor.input_dirty.clear();
    }

    pub fn reset_states(&mut self) {
        self.executor.reset_states();
    }

    // === Phase 1: compile ===

    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) {
        graph.validate_with(func_lib);

        self.plan.clear();

        // Flatten subgraphs straight into execution nodes — no intermediate
        // `Graph`. Everything below is boundary-agnostic (func nodes only).
        self.program.n_outputs = self.flattener.build(
            &mut self.program.e_nodes,
            flatten::Pools {
                inputs: &mut self.program.inputs,
                events: &mut self.program.events,
                input_dirty: &mut self.executor.input_dirty,
            },
            graph,
            func_lib,
            &mut self.flatten,
        ) as usize;

        // Realign the runtime cache to the rebuilt node set (preserve by id,
        // default new, trim gone).
        self.executor.reconcile(&self.program.e_nodes);

        self.validate_built(func_lib);
    }

    // === Phases 2–3: plan then execute ===

    pub async fn execute_terminals(&mut self) -> Result<ExecutionStats> {
        self.execute(true, false, [], None, None).await
    }

    pub async fn execute_events<T: IntoIterator<Item = EventRef>>(
        &mut self,
        events: T,
    ) -> Result<ExecutionStats> {
        self.execute(false, false, events, None, None).await
    }

    /// When `progress` is `Some`, a [`RunProgress`] is sent before and after
    /// each node's lambda runs, for live per-node feedback ahead of the final
    /// stats. When `cancel` is `Some` and gets set mid-run, scheduling stops
    /// after the in-flight node and the returned stats are marked `cancelled`.
    pub async fn execute<T: IntoIterator<Item = EventRef>>(
        &mut self,
        terminals: bool,
        event_triggers: bool,
        events: T,
        progress: Option<&UnboundedSender<RunProgress>>,
        cancel: Option<CancelToken>,
    ) -> Result<ExecutionStats> {
        let events: Vec<EventRef> = events.into_iter().collect();

        // Phase 2: schedule into the reusable plan buffer.
        self.planner.plan(
            &self.program,
            &self.executor,
            terminals,
            event_triggers,
            &events,
            &mut self.plan,
        )?;

        // Phase 3: run the schedule.
        let mut stats = self
            .executor
            .run(&self.program, &self.plan, &self.flatten, progress, cancel)
            .await;
        stats.triggered_events = events;
        // Annotate with how the graph was flattened so the stats' flat ids
        // can be projected back onto authoring nodes (the executor itself
        // stays oblivious to the authoring graph).
        stats.flatten = self.flatten.clone();

        Ok(stats)
    }

    // === Query ===

    pub fn get_argument_values(&self, node_id: &NodeId) -> Option<ArgumentValues> {
        let idx = self.program.e_nodes.index_of_key(node_id)?;
        let e_node = &self.program.e_nodes[idx];

        let inputs = self.program.inputs[e_node.inputs.range()]
            .iter()
            .map(|input| match &input.binding {
                ExecutionBinding::None => None,
                ExecutionBinding::Const(v) => Some(DynamicValue::from(v)),
                ExecutionBinding::Bind(addr) => self.executor.slots[addr.target_idx]
                    .output_values
                    .as_ref()
                    .and_then(|o| o.get(addr.port_idx))
                    .cloned(),
            })
            .collect();

        let outputs = self.executor.slots[idx]
            .output_values
            .as_ref()
            .map(|o| o.to_vec())
            .unwrap_or_default();

        Some(ArgumentValues { inputs, outputs })
    }

    /// `get_argument_values` plus awaited preview resolution.
    pub async fn get_argument_values_with_previews(
        &mut self,
        node_id: &NodeId,
    ) -> Option<ArgumentValues> {
        let mut values = self.get_argument_values(node_id)?;
        let mut pending_previews = Vec::new();
        for value in values
            .inputs
            .iter_mut()
            .flatten()
            .chain(values.outputs.iter_mut())
        {
            if let Some(pending) = value.gen_preview(&mut self.executor.ctx_manager) {
                pending_previews.push(pending);
            }
        }
        for pending in pending_previews {
            pending.wait(&mut self.executor.ctx_manager).await;
        }
        Some(values)
    }

    /// Collect every (event → lambda → state) triple that is currently
    /// "live" — node was executed or cached this run, the event has at
    /// least one subscriber, and its lambda is populated. Used by the
    /// worker to spawn the tasks that drive the event loop.
    pub fn active_event_triggers(&self, stats: &ExecutionStats) -> Vec<EventTrigger> {
        stats
            .cached_nodes
            .iter()
            .copied()
            .chain(stats.executed_nodes.iter().map(|n| n.node_id))
            .flat_map(|node_id| {
                let e_node = self.by_id(&node_id).unwrap();
                let event_state = self
                    .executor
                    .slots
                    .by_key(&node_id)
                    .unwrap()
                    .event_state
                    .clone();
                let id = e_node.id;
                self.program.events[e_node.events.range()]
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| !event.subscribers.is_empty() && !event.lambda.is_none())
                    .map(move |(event_idx, event)| EventTrigger {
                        event: EventRef {
                            node_id: id,
                            event_idx,
                        },
                        lambda: event.lambda.clone(),
                        state: event_state.clone(),
                    })
            })
            .collect()
    }

    // === Validation ===

    /// Self-consistency check of the compiled program against the `FuncLib`,
    /// plus that the runtime slots stayed index-aligned to the nodes after
    /// `reconcile`. The source graph is gone after flattening, so this
    /// validates each `e_node` against its func and checks binding integrity.
    fn validate_built(&self, func_lib: &FuncLib) {
        if !is_debug() {
            return;
        }

        // Runtime slots stay index-aligned to nodes after `reconcile`.
        assert_eq!(self.executor.slots.len(), self.program.e_nodes.len());

        let mut seen_node_ids: HashSet<NodeId> = HashSet::with_capacity(self.program.e_nodes.len());
        for (idx, e_node) in self.program.e_nodes.iter().enumerate() {
            assert!(seen_node_ids.insert(e_node.id));

            let slot = &self.executor.slots[idx];
            assert_eq!(slot.id, e_node.id);
            if let Some(output_values) = slot.output_values.as_ref() {
                assert_eq!(output_values.len(), e_node.outputs.len as usize);
            }

            let func = func_lib.by_id(&e_node.func_id).unwrap();
            assert_eq!(e_node.inputs.len as usize, func.inputs.len());
            assert_eq!(e_node.outputs.len as usize, func.outputs.len());
            assert_eq!(e_node.events.len as usize, func.events.len());

            for e_input in &self.program.inputs[e_node.inputs.range()] {
                if let ExecutionBinding::Bind(e_addr) = &e_input.binding {
                    assert!(e_addr.target_idx < self.program.e_nodes.len());
                    let target = &self.program.e_nodes[e_addr.target_idx];
                    assert_eq!(e_addr.target_id, target.id);
                    assert!(e_addr.port_idx < target.outputs.len as usize);
                }
            }
        }
    }
}

/// Test-only inspection of the last plan's per-run flags and the runtime
/// slots. Nothing in production reads per-run state off the engine — the
/// executor reads it straight from the live `ExecutionPlan`.
#[cfg(test)]
impl ExecutionEngine {
    /// Run only the planning phase (no execution), leaving the schedule in
    /// `self.plan` for inspection.
    pub(crate) fn prepare_execution(
        &mut self,
        terminals: bool,
        event_triggers: bool,
        events: &[EventRef],
    ) -> Result<()> {
        self.planner.plan(
            &self.program,
            &self.executor,
            terminals,
            event_triggers,
            events,
            &mut self.plan,
        )
    }

    pub(crate) fn by_name(&self, node_name: &str) -> Option<&ExecutionNode> {
        self.program
            .e_nodes
            .iter()
            .find(|node| node.name == node_name)
    }

    pub(crate) fn node_inputs(&self, e_node: &ExecutionNode) -> &[program::ExecutionInput] {
        &self.program.inputs[e_node.inputs.range()]
    }

    pub(crate) fn node_events(&self, e_node: &ExecutionNode) -> &[program::ExecutionEvent] {
        &self.program.events[e_node.events.range()]
    }

    pub(crate) fn node_flags(&self, e_node: &ExecutionNode) -> plan::NodeFlags {
        let idx = self.program.e_nodes.index_of_key(&e_node.id).unwrap();
        self.plan.node_flags[idx]
    }

    pub(crate) fn node_input_flags(&self, e_node: &ExecutionNode) -> &[plan::InputFlags] {
        &self.plan.input_flags[e_node.inputs.range()]
    }

    /// Cross-run per-input dirty bits for a node, from the executor.
    pub(crate) fn node_input_dirty(&self, e_node: &ExecutionNode) -> &[bool] {
        &self.executor.input_dirty[e_node.inputs.range()]
    }

    pub(crate) fn node_output_usage(&self, e_node: &ExecutionNode) -> &[u32] {
        &self.plan.output_usage[e_node.outputs.range()]
    }

    pub(crate) fn runtime_slot(&self, e_node: &ExecutionNode) -> &executor::RuntimeSlot {
        self.executor.slots.by_key(&e_node.id).unwrap()
    }

    /// Iterator over runtime slots, index-aligned to `e_nodes`.
    pub(crate) fn runtime_slots(&self) -> std::slice::Iter<'_, executor::RuntimeSlot> {
        self.executor.slots.iter()
    }

    /// Seed a node's cached output (simulating a prior run).
    pub(crate) fn set_output_values(&mut self, node_name: &str, values: Vec<DynamicValue>) {
        let id = self.by_name(node_name).unwrap().id;
        self.executor.slots.by_key_mut(&id).unwrap().output_values = Some(values);
    }
}
