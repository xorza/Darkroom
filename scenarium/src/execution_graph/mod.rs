use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use common::{BoolExt, SerdeFormat, is_debug};
use hashbrown::HashSet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::common::shared_any_state::SharedAnyState;
use crate::context::ContextManager;
use crate::data::{DataType, DynamicValue, StaticValue};
use crate::event_lambda::EventLambda;
use crate::execution_stats::{ExecutedNodeStats, ExecutionStats, NodeError};
use crate::func_lambda::InvokeInput;
use crate::function::{FuncBehavior, FuncLib};
use crate::graph::{Graph, InputPort, NodeBehavior, NodeId};
use crate::prelude::{AnyState, FuncId, FuncLambda};
use crate::worker::{EventRef, EventTrigger};

mod flatten;
#[cfg(test)]
mod tests;

// === Error Types ===

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum Error {
    #[error("{message}")]
    Invoke { func_id: FuncId, message: String },
    #[error("Cycle detected while building execution graph at node {node_id:?}")]
    CycleDetected { node_id: NodeId },
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
    Needed,
}

// === Execution Binding ===

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct ExecutionPortAddress {
    pub target_id: NodeId,
    pub target_idx: usize,
    pub port_idx: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub enum ExecutionBinding {
    #[default]
    None,
    Const(StaticValue),
    Bind(ExecutionPortAddress),
}

impl ExecutionBinding {
    pub fn as_bind(&self) -> Option<&ExecutionPortAddress> {
        match self {
            ExecutionBinding::Bind(addr) => Some(addr),
            _ => None,
        }
    }
}

// === Execution Node Components ===

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionInput {
    pub required: bool,
    /// Cross-run dirty bit: set by `flatten` when the binding changes at
    /// `update`, consumed/cleared by `run_execution`. Bridges update→run, so
    /// unlike the per-run input flags it stays on the input, not the plan.
    pub binding_changed: bool,
    pub binding: ExecutionBinding,
    pub data_type: DataType,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ExecutionEvent {
    pub subscribers: Vec<NodeId>,
    #[serde(skip, default)]
    pub lambda: EventLambda,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExecutionBehavior {
    #[default]
    Impure,
    Pure,
    Once,
}

/// A contiguous slice into one of `ExecutionGraph`'s SoA pools.
#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize)]
pub struct Span {
    start: u32,
    len: u32,
}

impl Span {
    fn range(self) -> std::ops::Range<usize> {
        self.start as usize..(self.start + self.len) as usize
    }
}

/// DFS coloring for the two backward passes. White = unvisited, Gray = on
/// stack (Done pushed, children pending), Black = children done. Lives
/// only within a single pass as a local `Vec<Color>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    White,
    Gray,
    Black,
}

// === Execution Node ===

/// Topology + code for one flat node. Immutable across runs; all mutable
/// per-run/cross-run state lives in the `ExecutionRuntime` slot of the same
/// index (see `RuntimeSlot`).
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: NodeId,
    inited: bool,

    pub terminal: bool,
    pub behavior: ExecutionBehavior,

    pub inputs: Span,
    pub outputs: Span,
    pub events: Span,

    pub func_id: FuncId,

    #[serde(skip)]
    pub lambda: FuncLambda,

    #[serde(default)]
    pub name: String,
}

impl KeyIndexKey<NodeId> for ExecutionNode {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

impl ExecutionNode {
    fn compute_behavior(
        node_behavior: NodeBehavior,
        func_behavior: FuncBehavior,
    ) -> ExecutionBehavior {
        match node_behavior {
            NodeBehavior::AsFunction => match func_behavior {
                FuncBehavior::Pure => ExecutionBehavior::Pure,
                FuncBehavior::Impure => ExecutionBehavior::Impure,
            },
            NodeBehavior::Once => ExecutionBehavior::Once,
        }
    }
}

// === Graph Traversal Helpers ===

#[derive(Debug)]
enum VisitCause {
    Terminal,
    OutputRequest { output_idx: usize },
    Done,
}

#[derive(Debug)]
struct Visit {
    e_node_idx: usize,
    cause: VisitCause,
}

/// Per-run scratch buffers, kept on the `ExecutionGraph` and reused across
/// runs so a repeated `execute_*` on an unchanged graph does no scheduling
/// allocations. Reset (clear/resize) each run, never freed.
#[derive(Debug, Default)]
struct Scratch {
    /// DFS coloring, reused across *both* backward passes (reset between).
    color: Vec<Color>,
    /// DFS work stack.
    stack: Vec<Visit>,
    /// Terminal-membership marker column (dedup without hashing).
    is_terminal: Vec<bool>,
    /// Deduped terminal node indices that seed the backward walks.
    terminal_seeds: Vec<usize>,
    /// Per-node invoke scratch, refilled per executed node.
    inputs: Vec<InvokeInput>,
    output_usage: Vec<OutputUsage>,
}

impl Scratch {
    /// Mark `idx` as a terminal seed, deduping via the marker column.
    fn mark_terminal(&mut self, idx: usize) {
        if !self.is_terminal[idx] {
            self.is_terminal[idx] = true;
            self.terminal_seeds.push(idx);
        }
    }
}

// === Execution Plan ===

/// Per-run scheduling state for one node, indexed by `e_node_idx`.
#[derive(Debug, Clone, Copy, Default)]
pub struct NodeFlags {
    pub wants_execute: bool,
    pub cached: bool,
    pub inputs_updated: bool,
    pub missing_required_inputs: bool,
}

/// Per-run scheduling state for one input, indexed by input-pool index.
#[derive(Debug, Clone, Copy, Default)]
pub struct InputFlags {
    pub dependency_wants_execute: bool,
    pub missing: bool,
}

/// The per-run schedule produced by `prepare_execution` and consumed by
/// `run_execution`: the two backward-walk orders plus the SoA per-run flag
/// columns (node/input/output), all keyed by the same indices as the
/// program's pools. Regenerated every run; reused via an internal buffer on
/// `ExecutionGraph` so a repeated run does no scheduling allocation.
#[derive(Debug, Default)]
pub struct ExecutionPlan {
    /// Post-order DFS over the dependency graph (deps before consumers),
    /// seeded from the terminals. Superset of `execute_order`.
    pub process_order: Vec<usize>,
    /// Pruned to only nodes whose output is read by an executing consumer.
    pub execute_order: Vec<usize>,
    /// Per-node flags, indexed by `e_node_idx`.
    node_flags: Vec<NodeFlags>,
    /// Per-input flags, indexed by input-pool index.
    input_flags: Vec<InputFlags>,
    /// Per-output consumer counts, indexed by output-pool index. `> 0` ⇒
    /// the output is Needed this run; `0` ⇒ Skip. A count (not a bool) so
    /// future refcount-based eviction can use the multiplicity.
    output_usage: Vec<u32>,
}

impl ExecutionPlan {
    fn clear(&mut self) {
        self.process_order.clear();
        self.execute_order.clear();
        self.node_flags.clear();
        self.input_flags.clear();
        self.output_usage.clear();
    }

    /// Clear the orders and reset every flag column to default at the given
    /// pool sizes. Called at the start of each `prepare_execution`.
    fn reset(&mut self, n_nodes: usize, n_inputs: usize, n_outputs: usize) {
        self.process_order.clear();
        self.execute_order.clear();

        self.node_flags.clear();
        self.node_flags.resize(n_nodes, NodeFlags::default());
        self.input_flags.clear();
        self.input_flags.resize(n_inputs, InputFlags::default());
        self.output_usage.clear();
        self.output_usage.resize(n_outputs, 0);
    }
}

// === Execution Runtime ===

/// Per-node runtime state, index-aligned to `e_nodes`: the cross-run value
/// cache (`state`, `event_state`, `output_values`) plus per-run results
/// (`error`, `run_time`). Kept off the program node so the program stays
/// immutable across a run; carries its own `id` so the cache can be
/// reconciled by key on `update` (surviving node reorder/trim).
#[derive(Default, Debug)]
pub(crate) struct RuntimeSlot {
    id: NodeId,
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

/// Mutable execution-time state: per-node `slots` (cache + per-run results,
/// index-aligned to `e_nodes`) and the shared `ctx_manager`. Not serialized.
#[derive(Default, Debug)]
pub(crate) struct ExecutionRuntime {
    slots: KeyIndexVec<NodeId, RuntimeSlot>,
    ctx_manager: ContextManager,
}

impl ExecutionRuntime {
    /// Rebuild `slots` in `e_nodes` order: preserve each surviving node's
    /// cache by id, default new nodes, trim removed ones. Mirrors the
    /// `CompactInsert` flatten runs over `e_nodes`, keeping `slots[i]` aligned
    /// to `e_nodes[i]`.
    fn reconcile(&mut self, e_nodes: &KeyIndexVec<NodeId, ExecutionNode>) {
        let mut compact = self.slots.compact_insert_start();
        for e_node in e_nodes.iter() {
            compact.insert_with(&e_node.id, || RuntimeSlot {
                id: e_node.id,
                ..Default::default()
            });
        }
    }
}

// === Execution Graph ===

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub e_nodes: KeyIndexVec<NodeId, ExecutionNode>,

    inputs: Vec<ExecutionInput>,
    events: Vec<ExecutionEvent>,
    /// Total output count across all nodes (sum of every node's output span
    /// length). Sizes the plan's `output_usage` column; outputs carry no
    /// per-node static data, so there is no output pool — only this count and
    /// the per-node `outputs` span.
    n_outputs: usize,

    /// Mutable execution-time state (cache + per-run results + context).
    #[serde(skip)]
    runtime: ExecutionRuntime,
    /// Reusable subgraph-flattening scratch (kept across updates).
    #[serde(skip)]
    flattener: flatten::Flattener,
    /// Reusable per-run scheduling scratch (kept across runs).
    #[serde(skip)]
    scratch: Scratch,
    /// Reusable plan buffer, recycled across runs to avoid reallocation.
    #[serde(skip)]
    plan_buf: ExecutionPlan,
}

impl ExecutionGraph {
    // === Accessors ===

    pub fn by_id(&self, node_id: &NodeId) -> Option<&ExecutionNode> {
        self.e_nodes.by_key(node_id)
    }

    pub fn is_empty(&self) -> bool {
        self.e_nodes.is_empty()
    }

    pub fn by_name(&self, node_name: &str) -> Option<&ExecutionNode> {
        self.e_nodes.iter().find(|node| node.name == node_name)
    }

    pub fn by_name_mut(&mut self, node_name: &str) -> Option<&mut ExecutionNode> {
        self.e_nodes.iter_mut().find(|node| node.name == node_name)
    }

    pub fn node_inputs(&self, e_node: &ExecutionNode) -> &[ExecutionInput] {
        &self.inputs[e_node.inputs.range()]
    }

    pub fn node_events(&self, e_node: &ExecutionNode) -> &[ExecutionEvent] {
        &self.events[e_node.events.range()]
    }

    // === Serialization ===

    pub fn serialize(&self, format: SerdeFormat) -> Vec<u8> {
        common::serialize(self, format)
    }

    pub fn deserialize(serialized: &[u8], format: SerdeFormat) -> anyhow::Result<Self> {
        common::deserialize(serialized, format)
    }

    // === State Management ===

    pub fn clear(&mut self) {
        self.e_nodes.clear();
        self.plan_buf.clear();
        self.inputs.clear();
        self.events.clear();
        self.n_outputs = 0;
        self.runtime.slots.clear();
    }

    pub fn reset_states(&mut self) {
        for slot in self.runtime.slots.iter_mut() {
            slot.reset_state();
        }
    }

    // === Graph Update ===

    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) {
        graph.validate_with(func_lib);

        self.plan_buf.clear();

        // Flatten subgraphs straight into execution nodes — no intermediate
        // `Graph`. Everything below is boundary-agnostic (func nodes only).
        self.flattener.build(
            &mut self.e_nodes,
            flatten::Pools {
                inputs: &mut self.inputs,
                events: &mut self.events,
                n_outputs: &mut self.n_outputs,
            },
            graph,
            func_lib,
        );

        // Realign the runtime cache to the rebuilt node set (preserve by id,
        // default new, trim gone).
        self.runtime.reconcile(&self.e_nodes);

        self.validate_built(func_lib);
    }

    // === Execution ===

    pub async fn execute_terminals(&mut self) -> Result<ExecutionStats> {
        self.execute(true, false, []).await
    }

    pub async fn execute_events<T: IntoIterator<Item = EventRef>>(
        &mut self,
        events: T,
    ) -> Result<ExecutionStats> {
        self.execute(false, false, events).await
    }

    pub async fn execute<T: IntoIterator<Item = EventRef>>(
        &mut self,
        terminals: bool,
        event_triggers: bool,
        events: T,
    ) -> Result<ExecutionStats> {
        let events: Vec<EventRef> = events.into_iter().collect();

        self.prepare_execution(terminals, event_triggers, &events)?;

        let mut stats = self.run_execution().await;
        stats.triggered_events = events;

        Ok(stats)
    }

    /// Build the per-run schedule into `self.plan_buf`. The only writer of
    /// `plan_buf`; `run_execution` is the only reader. This pair is the
    /// prepare→execute seam.
    fn prepare_execution(
        &mut self,
        terminals: bool,
        event_triggers: bool,
        events: &[EventRef],
    ) -> Result<()> {
        let mut plan = std::mem::take(&mut self.plan_buf);
        plan.reset(self.e_nodes.len(), self.inputs.len(), self.n_outputs);
        let mut scratch = std::mem::take(&mut self.scratch);

        self.collect_terminal_nodes(terminals, event_triggers, events, &mut scratch);

        for slot in self.runtime.slots.iter_mut() {
            slot.run_time = 0.0;
            slot.error = None;
        }

        let result = self.walk_backward_collect_order(
            &scratch.terminal_seeds,
            &mut scratch.stack,
            &mut scratch.color,
            &mut plan,
        );
        if result.is_ok() {
            self.propagate_input_state_forward(&mut plan);
            self.walk_backward_collect_execute_order(
                &scratch.terminal_seeds,
                &mut scratch.stack,
                &mut scratch.color,
                &mut plan,
            );
            self.validate_for_execution(&plan);
        }

        self.scratch = scratch;
        self.plan_buf = plan;
        result
    }

    fn collect_terminal_nodes(
        &self,
        terminals: bool,
        event_triggers: bool,
        events: &[EventRef],
        scratch: &mut Scratch,
    ) {
        scratch.is_terminal.clear();
        scratch.is_terminal.resize(self.e_nodes.len(), false);
        scratch.terminal_seeds.clear();

        // Add event subscribers
        for event in events {
            let e_node = self.e_nodes.by_key(&event.node_id).unwrap();
            let subs = self.events[e_node.events.range()][event.event_idx]
                .subscribers
                .clone();
            for sub in &subs {
                let idx = self.e_nodes.index_of_key(sub).unwrap();
                scratch.mark_terminal(idx);
            }
        }

        // Add terminal nodes
        if terminals {
            for (idx, e) in self.e_nodes.iter().enumerate() {
                if e.terminal {
                    scratch.mark_terminal(idx);
                }
            }
        }

        // Add nodes with event triggers
        if event_triggers {
            for (idx, e) in self.e_nodes.iter().enumerate() {
                if self.events[e.events.range()]
                    .iter()
                    .any(|ev| !ev.subscribers.is_empty())
                {
                    scratch.mark_terminal(idx);
                }
            }
        }
    }

    fn walk_backward_collect_order(
        &mut self,
        terminal_seeds: &[usize],
        stack: &mut Vec<Visit>,
        color: &mut Vec<Color>,
        plan: &mut ExecutionPlan,
    ) -> Result<()> {
        plan.process_order.clear();
        stack.clear();

        color.clear();
        color.resize(self.e_nodes.len(), Color::White);

        for e_node_idx in terminal_seeds.iter().copied() {
            stack.push(Visit {
                e_node_idx,
                cause: VisitCause::Terminal,
            });
        }

        while let Some(visit) = stack.pop() {
            match visit.cause {
                VisitCause::Terminal => {}
                VisitCause::OutputRequest { output_idx } => {
                    let span = self.e_nodes[visit.e_node_idx].outputs;
                    plan.output_usage[span.start as usize + output_idx] += 1;
                }
                VisitCause::Done => {
                    assert_eq!(color[visit.e_node_idx], Color::Gray);
                    color[visit.e_node_idx] = Color::Black;
                    plan.process_order.push(visit.e_node_idx);
                    continue;
                }
            }

            let idx = visit.e_node_idx;
            match color[idx] {
                Color::Gray => {
                    return Err(Error::CycleDetected {
                        node_id: self.e_nodes[idx].id,
                    });
                }
                Color::Black => continue,
                Color::White => {}
            }

            color[idx] = Color::Gray;
            stack.push(Visit {
                e_node_idx: idx,
                cause: VisitCause::Done,
            });

            let span = self.e_nodes[idx].inputs;
            for e_input in &self.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    stack.push(Visit {
                        e_node_idx: addr.target_idx,
                        cause: VisitCause::OutputRequest {
                            output_idx: addr.port_idx,
                        },
                    });
                }
            }
        }

        Ok(())
    }

    fn propagate_input_state_forward(&mut self, plan: &mut ExecutionPlan) {
        // Debug-only: verify every Bind dep was already processed in this
        // forward pass. Guaranteed by process_order being post-order DFS
        // (deps before consumers), but worth checking — if this flips, the
        // forward pass is reading a stale `wants_execute`/`missing_required`.
        let mut processed = if is_debug() {
            vec![false; self.e_nodes.len()]
        } else {
            Vec::new()
        };

        for order_idx in 0..plan.process_order.len() {
            let e_node_idx = plan.process_order[order_idx];
            let inputs_span = self.e_nodes[e_node_idx].inputs;

            let mut inputs_updated = false;
            let mut bindings_changed = false;
            let mut missing_required = false;

            for pool_idx in inputs_span.range() {
                let e_input = &self.inputs[pool_idx];
                let binding_changed = e_input.binding_changed;
                let (dep_wants_execute, missing) = match &e_input.binding {
                    ExecutionBinding::None => (false, e_input.required),
                    ExecutionBinding::Const(_) => (false, false),
                    ExecutionBinding::Bind(addr) => {
                        let target_idx = addr.target_idx;
                        assert!(addr.port_idx < self.e_nodes[target_idx].outputs.len as usize);
                        if is_debug() {
                            assert!(processed[target_idx], "forward pass: dep not yet processed");
                        }
                        let dep = plan.node_flags[target_idx];
                        (
                            dep.wants_execute,
                            e_input.required && dep.missing_required_inputs,
                        )
                    }
                };

                plan.input_flags[pool_idx] = InputFlags {
                    dependency_wants_execute: dep_wants_execute,
                    missing,
                };
                inputs_updated |= binding_changed || dep_wants_execute;
                bindings_changed |= binding_changed;
                missing_required |= missing;
            }

            let behavior = self.e_nodes[e_node_idx].behavior;
            let has_outputs = self.runtime.slots[e_node_idx].output_values.is_some();
            let flags = &mut plan.node_flags[e_node_idx];
            flags.inputs_updated = inputs_updated;
            flags.missing_required_inputs = missing_required;

            if missing_required {
                flags.wants_execute = false;
                flags.cached = false;
            } else if bindings_changed {
                flags.wants_execute = true;
                flags.cached = false;
            } else {
                flags.cached = match behavior {
                    ExecutionBehavior::Impure => false,
                    ExecutionBehavior::Pure => has_outputs && !inputs_updated,
                    ExecutionBehavior::Once => has_outputs,
                };
                flags.wants_execute = !flags.cached;
            }

            if is_debug() {
                processed[e_node_idx] = true;
            }
        }
    }

    // Prunes `process_order` to only nodes whose output is actually
    // read by an executing consumer this run. A filter over `wants_execute`
    // is not equivalent: a Pure/Impure node can have `wants_execute = true`
    // while its sole consumer is Once-cached and won't read it — the forward
    // pass can't see that because "needed by consumer" is a backward fact.
    // See `once_node_toggle_refreshes_upstream` in tests.rs for the case
    // this pass exists to handle.
    fn walk_backward_collect_execute_order(
        &mut self,
        terminal_seeds: &[usize],
        stack: &mut Vec<Visit>,
        color: &mut Vec<Color>,
        plan: &mut ExecutionPlan,
    ) {
        plan.execute_order.clear();
        stack.clear();

        color.clear();
        color.resize(self.e_nodes.len(), Color::White);

        for e_node_idx in terminal_seeds.iter().copied() {
            stack.push(Visit {
                e_node_idx,
                cause: VisitCause::Terminal,
            });
        }

        while let Some(visit) = stack.pop() {
            let idx = visit.e_node_idx;

            match visit.cause {
                VisitCause::Terminal | VisitCause::OutputRequest { .. } => {}
                VisitCause::Done => {
                    assert_eq!(color[idx], Color::Gray);
                    plan.execute_order.push(idx);
                    color[idx] = Color::Black;
                    continue;
                }
            }

            match color[idx] {
                Color::White => {}
                Color::Black => continue,
                // Pass 1 would have rejected any cycle; a Gray revisit in
                // pass 2 means our DFS invariant is broken.
                Color::Gray => unreachable!("cycle should be detected in pass 1"),
            }

            if !plan.node_flags[idx].wants_execute {
                color[idx] = Color::Black;
                continue;
            }

            color[idx] = Color::Gray;
            stack.push(Visit {
                e_node_idx: idx,
                cause: VisitCause::Done,
            });

            let span = self.e_nodes[idx].inputs;
            for (pool_idx, e_input) in self.inputs[span.range()].iter().enumerate() {
                if plan.input_flags[span.start as usize + pool_idx].dependency_wants_execute
                    && let Some(addr) = e_input.binding.as_bind()
                {
                    stack.push(Visit {
                        e_node_idx: addr.target_idx,
                        cause: VisitCause::OutputRequest {
                            output_idx: addr.port_idx,
                        },
                    });
                }
            }
        }
    }

    async fn run_execution(&mut self) -> ExecutionStats {
        let start = std::time::Instant::now();

        let plan = std::mem::take(&mut self.plan_buf);
        let mut scratch = std::mem::take(&mut self.scratch);

        for e_node_idx in plan.execute_order.iter().copied() {
            if self.e_nodes[e_node_idx].lambda.is_none() {
                continue;
            }

            let func_id = self.e_nodes[e_node_idx].func_id;

            if self.has_errored_dependency(e_node_idx) {
                let slot = &mut self.runtime.slots[e_node_idx];
                slot.output_values = None;
                slot.error = Some(Error::Invoke {
                    func_id,
                    message: "Skipped due to upstream error".to_string(),
                });
                continue;
            }

            self.collect_inputs(e_node_idx, &plan, &mut scratch.inputs);
            self.collect_output_usage(e_node_idx, &plan, &mut scratch.output_usage);

            let output_count = self.e_nodes[e_node_idx].outputs.len as usize;
            let event_state = self.runtime.slots[e_node_idx].event_state.clone();
            assert!(self.runtime.slots[e_node_idx].error.is_none());

            let start = std::time::Instant::now();
            let result = {
                let lambda = &self.e_nodes[e_node_idx].lambda;
                let ExecutionRuntime { slots, ctx_manager } = &mut self.runtime;
                let slot = &mut slots[e_node_idx];
                let outputs = slot
                    .output_values
                    .get_or_insert_with(|| vec![DynamicValue::None; output_count]);
                lambda
                    .invoke(
                        ctx_manager,
                        &mut slot.state,
                        &event_state,
                        &scratch.inputs,
                        &scratch.output_usage,
                        outputs,
                    )
                    .await
                    .map_err(|e| Error::Invoke {
                        func_id,
                        message: e.to_string(),
                    })
            };

            let slot = &mut self.runtime.slots[e_node_idx];
            slot.run_time = start.elapsed().as_secs_f64();
            if let Err(err) = result {
                slot.error = Some(err);
                slot.output_values = None;
            }

            let span = self.e_nodes[e_node_idx].inputs;
            for e_input in &mut self.inputs[span.range()] {
                e_input.binding_changed = false;
            }
        }

        let stats = self.collect_execution_stats(start, &plan);
        self.scratch = scratch;
        self.plan_buf = plan;
        stats
    }

    fn has_errored_dependency(&self, e_node_idx: usize) -> bool {
        let span = self.e_nodes[e_node_idx].inputs;
        self.inputs[span.range()].iter().any(|input| {
            matches!(&input.binding, ExecutionBinding::Bind(addr) if self.runtime.slots[addr.target_idx].error.is_some())
        })
    }

    fn collect_inputs(
        &self,
        e_node_idx: usize,
        plan: &ExecutionPlan,
        inputs: &mut Vec<InvokeInput>,
    ) {
        inputs.clear();
        let span = self.e_nodes[e_node_idx].inputs;
        for (i, input) in self.inputs[span.range()].iter().enumerate() {
            let value = match &input.binding {
                ExecutionBinding::None => DynamicValue::None,
                ExecutionBinding::Const(v) => v.into(),
                ExecutionBinding::Bind(addr) => {
                    let outputs = self.runtime.slots[addr.target_idx]
                        .output_values
                        .as_ref()
                        .expect("missing output values");
                    assert_eq!(
                        outputs.len(),
                        self.e_nodes[addr.target_idx].outputs.len as usize
                    );
                    outputs[addr.port_idx].clone()
                }
            };
            let dependency_wants_execute =
                plan.input_flags[span.start as usize + i].dependency_wants_execute;
            inputs.push(InvokeInput {
                changed: input.binding_changed || dependency_wants_execute,
                value,
            });
        }
    }

    fn collect_output_usage(
        &self,
        e_node_idx: usize,
        plan: &ExecutionPlan,
        usage: &mut Vec<OutputUsage>,
    ) {
        usage.clear();
        let span = self.e_nodes[e_node_idx].outputs;
        usage.extend(
            plan.output_usage[span.range()]
                .iter()
                .map(|&c| (c == 0).then_else(OutputUsage::Skip, OutputUsage::Needed)),
        );
    }

    fn collect_execution_stats(
        &self,
        start: std::time::Instant,
        plan: &ExecutionPlan,
    ) -> ExecutionStats {
        let mut executed_nodes = Vec::new();
        let mut missing_inputs = Vec::new();
        let mut cached_nodes = Vec::new();
        let mut node_errors = Vec::new();

        for &idx in &plan.execute_order {
            executed_nodes.push(ExecutedNodeStats {
                node_id: self.e_nodes[idx].id,
                elapsed_secs: self.runtime.slots[idx].run_time,
            });
        }

        for &idx in &plan.process_order {
            let e = &self.e_nodes[idx];
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
            if let Some(err) = &self.runtime.slots[idx].error {
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
        }
    }

    // === Query ===

    pub fn get_argument_values(&self, node_id: &NodeId) -> Option<ArgumentValues> {
        let idx = self.e_nodes.index_of_key(node_id)?;
        let e_node = &self.e_nodes[idx];

        let inputs = self.inputs[e_node.inputs.range()]
            .iter()
            .map(|input| match &input.binding {
                ExecutionBinding::None => None,
                ExecutionBinding::Const(v) => Some(DynamicValue::from(v)),
                ExecutionBinding::Bind(addr) => self.runtime.slots[addr.target_idx]
                    .output_values
                    .as_ref()
                    .and_then(|o| o.get(addr.port_idx))
                    .cloned(),
            })
            .collect();

        let outputs = self.runtime.slots[idx]
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
            if let Some(pending) = value.gen_preview(&mut self.runtime.ctx_manager) {
                pending_previews.push(pending);
            }
        }
        for pending in pending_previews {
            pending.wait(&mut self.runtime.ctx_manager).await;
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
                    .runtime
                    .slots
                    .by_key(&node_id)
                    .unwrap()
                    .event_state
                    .clone();
                let id = e_node.id;
                self.events[e_node.events.range()]
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

    /// Self-consistency check of the built (flattened) execution graph. Needs
    /// only the `FuncLib` — the source graph is gone after flattening, so this
    /// validates each `e_node` against its func and checks binding/edge
    /// integrity rather than cross-checking a source graph.
    fn validate_built(&self, func_lib: &FuncLib) {
        if !is_debug() {
            return;
        }

        // Runtime slots stay index-aligned to nodes after `reconcile`.
        assert_eq!(self.runtime.slots.len(), self.e_nodes.len());

        let mut seen_node_ids: HashSet<NodeId> = HashSet::with_capacity(self.e_nodes.len());
        for (idx, e_node) in self.e_nodes.iter().enumerate() {
            assert!(seen_node_ids.insert(e_node.id));

            let slot = &self.runtime.slots[idx];
            assert_eq!(slot.id, e_node.id);
            if let Some(output_values) = slot.output_values.as_ref() {
                assert_eq!(output_values.len(), e_node.outputs.len as usize);
            }

            let func = func_lib.by_id(&e_node.func_id).unwrap();
            assert_eq!(e_node.inputs.len as usize, func.inputs.len());
            assert_eq!(e_node.outputs.len as usize, func.outputs.len());
            assert_eq!(e_node.events.len as usize, func.events.len());

            for e_input in &self.inputs[e_node.inputs.range()] {
                if let ExecutionBinding::Bind(e_addr) = &e_input.binding {
                    assert!(e_addr.target_idx < self.e_nodes.len());
                    let target = &self.e_nodes[e_addr.target_idx];
                    assert_eq!(e_addr.target_id, target.id);
                    assert!(e_addr.port_idx < target.outputs.len as usize);
                }
            }
        }
    }

    fn validate_for_execution(&self, plan: &ExecutionPlan) {
        if !is_debug() {
            return;
        }

        assert!(plan.process_order.len() <= self.e_nodes.len());

        // `process_order` is a post-order DFS: unique, and every Bind dep
        // appears before its consumer.
        let mut seen_in_order = HashSet::with_capacity(self.e_nodes.len());
        for &idx in &plan.process_order {
            assert!(idx < self.e_nodes.len());
            let span = self.e_nodes[idx].inputs;
            for input in &self.inputs[span.range()] {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    assert!(addr.target_idx < self.e_nodes.len());
                    assert!(seen_in_order.contains(&addr.target_idx));
                }
            }
            assert!(seen_in_order.insert(idx));
        }

        for (idx, e_node) in self.e_nodes.iter().enumerate() {
            let flags = plan.node_flags[idx];
            if flags.missing_required_inputs {
                assert!(!flags.wants_execute);
            }

            for e_input in &self.inputs[e_node.inputs.range()] {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    assert!(addr.target_idx < self.e_nodes.len());
                    assert!(addr.port_idx < self.e_nodes[addr.target_idx].outputs.len as usize);
                }
            }
        }

        assert!(plan.execute_order.len() <= plan.process_order.len());

        let mut pending: HashSet<usize> = plan.execute_order.iter().copied().collect();
        assert_eq!(pending.len(), plan.execute_order.len());

        for &idx in &plan.execute_order {
            assert!(idx < self.e_nodes.len());
            pending.remove(&idx);

            let e_node = &self.e_nodes[idx];
            let flags = plan.node_flags[idx];
            assert!(flags.wants_execute);
            assert!(!flags.missing_required_inputs);

            for e_input in &self.inputs[e_node.inputs.range()] {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    assert!(!pending.contains(&addr.target_idx));
                }
            }
        }
    }
}

/// Test-only inspection of the last plan's per-run flags. Nothing in
/// production reads per-run state off the graph — the executor reads it
/// straight from the live `ExecutionPlan`.
#[cfg(test)]
impl ExecutionGraph {
    pub(crate) fn node_flags(&self, e_node: &ExecutionNode) -> NodeFlags {
        let idx = self.e_nodes.index_of_key(&e_node.id).unwrap();
        self.plan_buf.node_flags[idx]
    }

    pub(crate) fn node_input_flags(&self, e_node: &ExecutionNode) -> &[InputFlags] {
        &self.plan_buf.input_flags[e_node.inputs.range()]
    }

    pub(crate) fn node_output_usage(&self, e_node: &ExecutionNode) -> &[u32] {
        &self.plan_buf.output_usage[e_node.outputs.range()]
    }

    // === Runtime-slot inspection ===

    pub(crate) fn runtime_slot(&self, e_node: &ExecutionNode) -> &RuntimeSlot {
        self.runtime.slots.by_key(&e_node.id).unwrap()
    }

    /// Iterator over runtime slots, index-aligned to `e_nodes`.
    pub(crate) fn runtime_slots(&self) -> std::slice::Iter<'_, RuntimeSlot> {
        self.runtime.slots.iter()
    }

    /// Seed a node's cached output (simulating a prior run).
    pub(crate) fn set_output_values(&mut self, node_name: &str, values: Vec<DynamicValue>) {
        let id = self.by_name(node_name).unwrap().id;
        self.runtime.slots.by_key_mut(&id).unwrap().output_values = Some(values);
    }
}
