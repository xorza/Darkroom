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
use crate::function::{Func, FuncBehavior, FuncLib};
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
    pub binding_changed: bool,
    pub dependency_wants_execute: bool,
    pub missing: bool,
    pub binding: ExecutionBinding,
    pub data_type: DataType,
}

// Count (not bool) — only `> 0` is read today for Skip/Needed, but tests
// assert exact counts and future change-pruning / refcount-based eviction
// will want the multiplicity.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ExecutionOutput {
    usage_count: usize,
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

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: NodeId,
    inited: bool,

    pub terminal: bool,
    pub missing_required_inputs: bool,
    pub wants_execute: bool,
    pub cached: bool,
    pub inputs_updated: bool,
    pub bindings_changed: bool,
    pub behavior: ExecutionBehavior,

    pub inputs: Vec<ExecutionInput>,
    pub outputs: Vec<ExecutionOutput>,
    pub events: Vec<ExecutionEvent>,

    pub func_id: FuncId,
    pub run_time: f64,
    pub error: Option<Error>,

    #[serde(skip)]
    pub(crate) state: AnyState,
    #[serde(skip)]
    pub(crate) event_state: SharedAnyState,
    #[serde(skip)]
    pub(crate) output_values: Option<Vec<DynamicValue>>,
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
    fn reset_for_execution(&mut self) {
        self.wants_execute = false;
        self.inputs_updated = false;
        self.bindings_changed = false;
        self.missing_required_inputs = false;
        self.cached = false;
        self.run_time = 0.0;
        self.error = None;

        for e_input in &mut self.inputs {
            e_input.dependency_wants_execute = false;
        }
    }

    fn reset_state(&mut self) {
        self.state = AnyState::default();
        self.event_state = SharedAnyState::default();
        self.output_values = None;
    }

    /// Refresh this (flattened) node from its func + the interior node's
    /// behavior/name. Event subscribers are cleared here and re-added by the
    /// flatten sink (already id-remapped), so it takes no node.
    fn refresh(&mut self, func: &Func, behavior: NodeBehavior, name: &str) {
        if !self.inited {
            self.init_from_func(func);
        } else {
            assert_eq!(self.func_id, func.id, "func changed under a reused node id");
            self.outputs.fill(ExecutionOutput::default());
        }

        self.terminal = func.terminal;
        self.behavior = Self::compute_behavior(behavior, func.behavior);

        for event in &mut self.events {
            event.subscribers.clear();
        }

        assert_eq!(self.inputs.len(), func.inputs.len());
        assert_eq!(self.outputs.len(), func.outputs.len());
        assert_eq!(self.events.len(), func.events.len());

        self.name.clear();
        self.name.push_str(name);
    }

    fn init_from_func(&mut self, func: &Func) {
        self.inited = true;
        self.func_id = func.id;
        self.lambda = func.lambda.clone();

        self.inputs
            .resize(func.inputs.len(), ExecutionInput::default());
        for (input_idx, func_input) in func.inputs.iter().enumerate() {
            let e_input = &mut self.inputs[input_idx];
            e_input.required = func_input.required;
            if e_input.data_type != func_input.data_type {
                e_input.data_type = func_input.data_type.clone();
            }
        }

        self.events.clear();
        self.events.reserve(func.events.len());
        for func_event in &func.events {
            self.events.push(ExecutionEvent {
                lambda: func_event.event_lambda.clone(),
                ..Default::default()
            });
        }

        self.outputs.clear();
        self.outputs
            .resize(func.outputs.len(), ExecutionOutput::default());
    }

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

// === Execution Graph ===

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub e_nodes: KeyIndexVec<NodeId, ExecutionNode>,
    pub e_node_process_order: Vec<usize>,
    pub e_node_execute_order: Vec<usize>,

    #[serde(skip)]
    ctx_manager: ContextManager,
    /// Reusable subgraph-flattening scratch (kept across updates).
    #[serde(skip)]
    flattener: flatten::Flattener,
    /// Reusable per-run scheduling scratch (kept across runs).
    #[serde(skip)]
    scratch: Scratch,
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
        self.e_node_process_order.clear();
        self.e_node_execute_order.clear();
    }

    pub fn reset_states(&mut self) {
        for e_node in &mut self.e_nodes {
            e_node.reset_state();
        }
    }

    // === Graph Update ===

    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) {
        graph.validate_with(func_lib);

        self.e_node_execute_order.clear();
        self.e_node_process_order.clear();

        // Flatten subgraphs straight into execution nodes — no intermediate
        // `Graph`. Everything below is boundary-agnostic (func nodes only).
        self.flattener.build(&mut self.e_nodes, graph, func_lib);

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

    fn prepare_execution(
        &mut self,
        terminals: bool,
        event_triggers: bool,
        events: &[EventRef],
    ) -> Result<()> {
        let mut scratch = std::mem::take(&mut self.scratch);

        self.collect_terminal_nodes(terminals, event_triggers, events, &mut scratch);

        self.e_nodes
            .iter_mut()
            .for_each(|e| e.reset_for_execution());

        let result = self.walk_backward_collect_order(
            &scratch.terminal_seeds,
            &mut scratch.stack,
            &mut scratch.color,
        );
        if result.is_ok() {
            self.propagate_input_state_forward();
            self.walk_backward_collect_execute_order(
                &scratch.terminal_seeds,
                &mut scratch.stack,
                &mut scratch.color,
            );
            self.validate_for_execution();
        }

        self.scratch = scratch;
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
            for sub in &e_node.events[event.event_idx].subscribers {
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
                if e.events.iter().any(|ev| !ev.subscribers.is_empty()) {
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
    ) -> Result<()> {
        self.e_node_process_order.clear();
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
                    self.e_nodes[visit.e_node_idx].outputs[output_idx].usage_count += 1;
                }
                VisitCause::Done => {
                    assert_eq!(color[visit.e_node_idx], Color::Gray);
                    color[visit.e_node_idx] = Color::Black;
                    self.e_node_process_order.push(visit.e_node_idx);
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

            for e_input in self.e_nodes[idx].inputs.iter() {
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

    fn propagate_input_state_forward(&mut self) {
        // Debug-only: verify every Bind dep was already processed in this
        // forward pass. Guaranteed by process_order being post-order DFS
        // (deps before consumers), but worth checking — if this flips, the
        // forward pass is reading a stale `wants_execute`/`missing_required`.
        let mut processed = if is_debug() {
            vec![false; self.e_nodes.len()]
        } else {
            Vec::new()
        };

        for e_node_idx in self.e_node_process_order.iter().copied() {
            let e_node = &self.e_nodes[e_node_idx];

            let mut inputs_updated = false;
            let mut bindings_changed = false;
            let mut missing_required = false;

            for input_idx in 0..e_node.inputs.len() {
                let e_input = &self.e_nodes[e_node_idx].inputs[input_idx];
                let (dep_wants_execute, missing) = match &e_input.binding {
                    ExecutionBinding::None => (false, e_input.required),
                    ExecutionBinding::Const(_) => (false, false),
                    ExecutionBinding::Bind(addr) => {
                        let output_node = &self.e_nodes[addr.target_idx];
                        assert!(addr.port_idx < output_node.outputs.len());
                        if is_debug() {
                            assert!(
                                processed[addr.target_idx],
                                "forward pass: dep not yet processed"
                            );
                        }
                        (
                            output_node.wants_execute,
                            e_input.required && output_node.missing_required_inputs,
                        )
                    }
                };

                let e_input = &mut self.e_nodes[e_node_idx].inputs[input_idx];
                e_input.dependency_wants_execute = dep_wants_execute;
                e_input.missing = missing;
                inputs_updated |= e_input.binding_changed || dep_wants_execute;
                bindings_changed |= e_input.binding_changed;
                missing_required |= missing;
            }

            let e_node = &mut self.e_nodes[e_node_idx];
            e_node.inputs_updated = inputs_updated;
            e_node.bindings_changed = bindings_changed;
            e_node.missing_required_inputs = missing_required;

            if missing_required {
                e_node.wants_execute = false;
                e_node.cached = false;
            } else if bindings_changed {
                e_node.wants_execute = true;
                e_node.cached = false;
            } else {
                e_node.cached = match e_node.behavior {
                    ExecutionBehavior::Impure => false,
                    ExecutionBehavior::Pure => e_node.output_values.is_some() && !inputs_updated,
                    ExecutionBehavior::Once => e_node.output_values.is_some(),
                };
                e_node.wants_execute = !e_node.cached;
            }

            if is_debug() {
                processed[e_node_idx] = true;
            }
        }
    }

    // Prunes `e_node_process_order` to only nodes whose output is actually
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
    ) {
        self.e_node_execute_order.clear();
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
                    self.e_node_execute_order.push(idx);
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

            if !self.e_nodes[idx].wants_execute {
                color[idx] = Color::Black;
                continue;
            }

            color[idx] = Color::Gray;
            stack.push(Visit {
                e_node_idx: idx,
                cause: VisitCause::Done,
            });

            for e_input in self.e_nodes[idx].inputs.iter() {
                if e_input.dependency_wants_execute
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

        let mut scratch = std::mem::take(&mut self.scratch);

        for e_node_idx in self.e_node_execute_order.iter().copied() {
            let e_node = &self.e_nodes[e_node_idx];
            if e_node.lambda.is_none() {
                continue;
            }

            if self.has_errored_dependency(e_node_idx) {
                let e_node = &mut self.e_nodes[e_node_idx];
                e_node.output_values = None;
                e_node.error = Some(Error::Invoke {
                    func_id: e_node.func_id,
                    message: "Skipped due to upstream error".to_string(),
                });
                continue;
            }

            self.collect_inputs(e_node_idx, &mut scratch.inputs);
            self.collect_output_usage(e_node_idx, &mut scratch.output_usage);

            let event_state = self.e_nodes[e_node_idx].event_state.clone();
            let e_node = &mut self.e_nodes[e_node_idx];
            assert!(e_node.error.is_none());

            let outputs = e_node
                .output_values
                .get_or_insert_with(|| vec![DynamicValue::None; e_node.outputs.len()]);

            let start = std::time::Instant::now();
            let result = e_node
                .lambda
                .invoke(
                    &mut self.ctx_manager,
                    &mut e_node.state,
                    &event_state,
                    &scratch.inputs,
                    &scratch.output_usage,
                    outputs,
                )
                .await
                .map_err(|e| Error::Invoke {
                    func_id: e_node.func_id,
                    message: e.to_string(),
                });

            e_node.run_time = start.elapsed().as_secs_f64();

            for e_input in &mut e_node.inputs {
                e_input.binding_changed = false;
            }

            if let Err(err) = result {
                e_node.error = Some(err);
                e_node.output_values = None;
            }
        }

        let stats = self.collect_execution_stats(start);
        self.scratch = scratch;
        stats
    }

    fn has_errored_dependency(&self, e_node_idx: usize) -> bool {
        self.e_nodes[e_node_idx].inputs.iter().any(|input| {
            matches!(&input.binding, ExecutionBinding::Bind(addr) if self.e_nodes[addr.target_idx].error.is_some())
        })
    }

    fn collect_inputs(&self, e_node_idx: usize, inputs: &mut Vec<InvokeInput>) {
        inputs.clear();
        for input in self.e_nodes[e_node_idx].inputs.iter() {
            let value = match &input.binding {
                ExecutionBinding::None => DynamicValue::None,
                ExecutionBinding::Const(v) => v.into(),
                ExecutionBinding::Bind(addr) => {
                    let output_node = &self.e_nodes[addr.target_idx];
                    let outputs = output_node
                        .output_values
                        .as_ref()
                        .expect("missing output values");
                    assert_eq!(outputs.len(), output_node.outputs.len());
                    outputs[addr.port_idx].clone()
                }
            };
            inputs.push(InvokeInput {
                changed: input.binding_changed || input.dependency_wants_execute,
                value,
            });
        }
    }

    fn collect_output_usage(&self, e_node_idx: usize, usage: &mut Vec<OutputUsage>) {
        usage.clear();
        usage.extend(
            self.e_nodes[e_node_idx]
                .outputs
                .iter()
                .map(|o| (o.usage_count == 0).then_else(OutputUsage::Skip, OutputUsage::Needed)),
        );
    }

    fn collect_execution_stats(&self, start: std::time::Instant) -> ExecutionStats {
        let mut executed_nodes = Vec::new();
        let mut missing_inputs = Vec::new();
        let mut cached_nodes = Vec::new();
        let mut node_errors = Vec::new();

        for &idx in &self.e_node_execute_order {
            let e = &self.e_nodes[idx];
            executed_nodes.push(ExecutedNodeStats {
                node_id: e.id,
                elapsed_secs: e.run_time,
            });
        }

        for &idx in &self.e_node_process_order {
            let e = &self.e_nodes[idx];
            if e.missing_required_inputs {
                for (i, inp) in e.inputs.iter().enumerate() {
                    if inp.missing {
                        missing_inputs.push(InputPort {
                            node_id: e.id,
                            port_idx: i,
                        });
                    }
                }
            }
            if e.cached {
                cached_nodes.push(e.id);
            }
            if let Some(err) = &e.error {
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
        let e_node = self.e_nodes.by_key(node_id)?;

        let inputs = e_node
            .inputs
            .iter()
            .map(|input| match &input.binding {
                ExecutionBinding::None => None,
                ExecutionBinding::Const(v) => Some(DynamicValue::from(v)),
                ExecutionBinding::Bind(addr) => self.e_nodes[addr.target_idx]
                    .output_values
                    .as_ref()
                    .and_then(|o| o.get(addr.port_idx))
                    .cloned(),
            })
            .collect();

        let outputs = e_node
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
            if let Some(pending) = value.gen_preview(&mut self.ctx_manager) {
                pending_previews.push(pending);
            }
        }
        for pending in pending_previews {
            pending.wait(&mut self.ctx_manager).await;
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
                let event_state = e_node.event_state.clone();
                e_node
                    .events
                    .iter()
                    .enumerate()
                    .filter(|(_, event)| !event.subscribers.is_empty() && !event.lambda.is_none())
                    .map(move |(event_idx, event)| EventTrigger {
                        event: EventRef {
                            node_id: e_node.id,
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

        assert!(self.e_node_process_order.len() <= self.e_nodes.len());

        let mut seen_node_ids: HashSet<NodeId> = HashSet::with_capacity(self.e_nodes.len());
        for e_node in self.e_nodes.iter() {
            assert!(seen_node_ids.insert(e_node.id));

            if let Some(output_values) = e_node.output_values.as_ref() {
                assert_eq!(output_values.len(), e_node.outputs.len());
            }

            let func = func_lib.by_id(&e_node.func_id).unwrap();
            assert_eq!(e_node.inputs.len(), func.inputs.len());
            assert_eq!(e_node.outputs.len(), func.outputs.len());
            assert_eq!(e_node.events.len(), func.events.len());

            for e_input in e_node.inputs.iter() {
                if let ExecutionBinding::Bind(e_addr) = &e_input.binding {
                    assert!(e_addr.target_idx < self.e_nodes.len());
                    let target = &self.e_nodes[e_addr.target_idx];
                    assert_eq!(e_addr.target_id, target.id);
                    assert!(e_addr.port_idx < target.outputs.len());
                }
            }
        }

        let mut seen = HashSet::with_capacity(self.e_nodes.len());
        for &idx in &self.e_node_process_order {
            assert!(idx < self.e_nodes.len());
            assert!(seen.insert(idx));
        }

        let mut seen_in_order = HashSet::with_capacity(self.e_nodes.len());
        for &idx in &self.e_node_process_order {
            assert!(idx < self.e_nodes.len());
            for input in self.e_nodes[idx].inputs.iter() {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    assert!(addr.target_idx < self.e_nodes.len());
                    assert!(seen_in_order.contains(&addr.target_idx));
                }
            }
            assert!(seen_in_order.insert(idx));
        }
    }

    fn validate_for_execution(&self) {
        if !is_debug() {
            return;
        }

        for e_node in self.e_nodes.iter() {
            if e_node.missing_required_inputs {
                assert!(!e_node.wants_execute);
            }

            for e_input in e_node.inputs.iter() {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    assert!(addr.target_idx < self.e_nodes.len());
                    assert!(addr.port_idx < self.e_nodes[addr.target_idx].outputs.len());
                }
            }
        }

        assert!(self.e_node_execute_order.len() <= self.e_node_process_order.len());

        let mut pending: HashSet<usize> = self.e_node_execute_order.iter().copied().collect();
        assert_eq!(pending.len(), self.e_node_execute_order.len());

        for &idx in &self.e_node_execute_order {
            assert!(idx < self.e_nodes.len());
            pending.remove(&idx);

            let e_node = &self.e_nodes[idx];
            assert!(e_node.wants_execute);
            assert!(!e_node.missing_required_inputs);

            for e_input in e_node.inputs.iter() {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    assert!(!pending.contains(&addr.target_idx));
                }
            }
        }
    }
}
