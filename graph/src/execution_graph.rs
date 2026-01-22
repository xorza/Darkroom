use std::panic;

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
use crate::graph::{Binding, Graph, Node, NodeBehavior, NodeId, PortAddress};
use crate::prelude::{AnyState, FuncId, FuncLambda};
use crate::worker::EventRef;

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

#[derive(Debug, Clone, Default, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputState {
    #[default]
    Changed,
    Unchanged,
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
    Undefined,
    None,
    Const(StaticValue),
    Bind(ExecutionPortAddress),
}

impl ExecutionBinding {
    pub fn as_const(&self) -> Option<&StaticValue> {
        match self {
            ExecutionBinding::Const(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_bind(&self) -> Option<&ExecutionPortAddress> {
        match self {
            ExecutionBinding::Bind(addr) => Some(addr),
            _ => None,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, ExecutionBinding::None)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum ProcessState {
    #[default]
    None,
    Processing,
    Backward,
    Forward,
}

// === Execution Node ===

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: NodeId,
    inited: bool,
    process_state: ProcessState,

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

    #[cfg(debug_assertions)]
    #[serde(default)]
    pub name: String,
}

impl KeyIndexKey<NodeId> for ExecutionNode {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

impl ExecutionNode {
    fn invalidate(&mut self) {
        self.output_values = None;
        self.process_state = ProcessState::None;
        self.inputs.clear();
        self.outputs.clear();
        self.reset_for_execution();
    }

    fn reset_for_execution(&mut self) {
        self.wants_execute = false;
        self.inputs_updated = false;
        self.bindings_changed = false;
        self.missing_required_inputs = false;
        self.cached = false;
        self.run_time = 0.0;
        self.error = None;
        self.process_state = ProcessState::Forward;

        for e_input in &mut self.inputs {
            e_input.dependency_wants_execute = false;
        }
    }

    fn reset_state(&mut self) {
        self.state = AnyState::default();
        self.event_state = SharedAnyState::default();
        self.output_values = None;
    }

    fn refresh(&mut self, node: &Node, func: &Func) {
        assert_eq!(self.id, node.id);
        assert_eq!(node.func_id, func.id);
        assert_eq!(node.inputs.len(), func.inputs.len());

        if !self.inited {
            self.init_from_func(func);
        } else {
            self.outputs.fill(ExecutionOutput::default());
        }

        self.terminal = func.terminal;
        self.process_state = ProcessState::None;
        self.behavior = Self::compute_behavior(node.behavior, func.behavior);

        for (event_idx, event) in node.events.iter().enumerate() {
            self.events[event_idx].subscribers.clear();
            self.events[event_idx]
                .subscribers
                .extend(&event.subscribers);
        }

        assert_eq!(self.inputs.len(), node.inputs.len());
        assert_eq!(self.outputs.len(), func.outputs.len());
        assert_eq!(self.events.len(), func.events.len());

        #[cfg(debug_assertions)]
        {
            self.name.clear();
            self.name.push_str(&node.name);
        }
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

// === Execution Graph ===

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub e_nodes: KeyIndexVec<NodeId, ExecutionNode>,
    pub e_node_process_order: Vec<usize>,
    pub e_node_execute_order: Vec<usize>,
    pub e_node_terminal_idx: HashSet<usize>,

    #[serde(skip)]
    pub ctx_manager: ContextManager,
    #[serde(skip)]
    stack: Vec<Visit>,
}

impl ExecutionGraph {
    // === Accessors ===

    pub fn by_id(&self, node_id: &NodeId) -> Option<&ExecutionNode> {
        self.e_nodes.by_key(node_id)
    }

    pub fn by_id_mut(&mut self, node_id: &NodeId) -> Option<&mut ExecutionNode> {
        self.e_nodes.by_key_mut(node_id)
    }

    #[cfg(debug_assertions)]
    pub fn by_name(&self, node_name: &str) -> Option<&ExecutionNode> {
        self.e_nodes.iter().find(|node| node.name == node_name)
    }

    #[cfg(debug_assertions)]
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
        self.e_node_terminal_idx.clear();
        self.stack.clear();
    }

    pub fn reset_states(&mut self) {
        for e_node in &mut self.e_nodes {
            e_node.reset_state();
        }
    }

    pub fn invalidate_recursively<I>(&mut self, node_ids: I)
    where
        I: IntoIterator<Item = NodeId>,
    {
        let mut stack: Vec<usize> = node_ids
            .into_iter()
            .filter_map(|id| self.e_nodes.index_of_key(&id))
            .collect();
        let mut seen = vec![false; self.e_nodes.len()];

        while let Some(e_node_idx) = stack.pop() {
            if seen[e_node_idx] {
                continue;
            }
            seen[e_node_idx] = true;
            self.e_nodes[e_node_idx].invalidate();

            for (idx, e_node) in self.e_nodes.iter().enumerate() {
                if seen[idx] {
                    continue;
                }
                let depends = e_node.inputs.iter().any(|input| {
                    matches!(&input.binding, ExecutionBinding::Bind(addr) if addr.target_idx == e_node_idx)
                });
                if depends {
                    stack.push(idx);
                }
            }
        }
    }

    // === Graph Update ===

    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) {
        graph.validate_with(func_lib);

        self.e_node_execute_order.clear();
        self.e_node_process_order.clear();

        self.build_execution_nodes(graph, func_lib);
        self.validate_with(graph, func_lib);
    }

    fn build_execution_nodes(&mut self, graph: &Graph, func_lib: &FuncLib) {
        self.e_nodes
            .iter_mut()
            .for_each(|e| e.process_state = ProcessState::None);

        let mut compact = self.e_nodes.compact_insert_start();

        for node in graph.nodes.iter() {
            let (e_node_idx, e_node) = compact.insert_with(&node.id, || ExecutionNode {
                id: node.id,
                ..Default::default()
            });

            assert_eq!(e_node.process_state, ProcessState::None);
            let node = graph.by_id(&e_node.id).unwrap();
            let func = func_lib.by_id(&node.func_id).unwrap();
            e_node.refresh(node, func);
            e_node.process_state = ProcessState::Forward;

            for (input_idx, input) in node.inputs.iter().enumerate() {
                Self::update_input_binding(&mut compact, e_node_idx, input_idx, &input.binding);
            }
        }
    }

    fn update_input_binding(
        compact: &mut common::key_index_vec::CompactInsert<'_, NodeId, ExecutionNode>,
        e_node_idx: usize,
        input_idx: usize,
        binding: &Binding,
    ) {
        let e_input = &mut compact[e_node_idx].inputs[input_idx];

        e_input.binding_changed |= match (binding, &e_input.binding) {
            (Binding::None, ExecutionBinding::None) => false,
            (Binding::None, _) => {
                e_input.binding = ExecutionBinding::None;
                true
            }
            (Binding::Const(v), ExecutionBinding::Const(existing)) if v == existing => false,
            (Binding::Const(v), _) => {
                e_input.binding = ExecutionBinding::Const(v.clone());
                true
            }
            (Binding::Bind(_), ExecutionBinding::Bind(_)) => false,
            (Binding::Bind(_), _) => {
                e_input.binding = ExecutionBinding::Undefined;
                true
            }
        };

        let Binding::Bind(port_address) = binding else {
            return;
        };

        let (output_e_node_idx, _) =
            compact.insert_with(&port_address.target_id, || ExecutionNode {
                id: port_address.target_id,
                ..Default::default()
            });

        let e_input = &mut compact[e_node_idx].inputs[input_idx];
        let desired = ExecutionPortAddress {
            target_id: port_address.target_id,
            target_idx: output_e_node_idx,
            port_idx: port_address.port_idx,
        };

        match &mut e_input.binding {
            ExecutionBinding::Bind(existing)
                if existing.target_id == desired.target_id
                    && existing.port_idx == desired.port_idx =>
            {
                existing.target_idx = desired.target_idx;
            }
            ExecutionBinding::Bind(existing) => {
                e_input.binding_changed = true;
                *existing = desired;
            }
            _ => {
                e_input.binding_changed = true;
                e_input.binding = ExecutionBinding::Bind(desired);
            }
        }

        assert!(!matches!(e_input.binding, ExecutionBinding::Undefined));
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

        self.e_node_terminal_idx.clear();

        Ok(stats)
    }

    fn prepare_execution(
        &mut self,
        terminals: bool,
        event_triggers: bool,
        events: &[EventRef],
    ) -> Result<()> {
        self.collect_terminal_nodes(terminals, event_triggers, events);

        self.e_nodes
            .iter_mut()
            .for_each(|e| e.reset_for_execution());

        self.walk_backward_collect_order()?;
        self.propagate_input_state_forward();
        self.walk_backward_collect_execute_order();

        self.validate_for_execution();

        Ok(())
    }

    fn collect_terminal_nodes(
        &mut self,
        terminals: bool,
        event_triggers: bool,
        events: &[EventRef],
    ) {
        self.e_node_terminal_idx.clear();

        // Add event subscribers
        self.e_node_terminal_idx
            .extend(events.iter().flat_map(|event| {
                self.e_nodes.by_key(&event.node_id).unwrap().events[event.event_idx]
                    .subscribers
                    .iter()
                    .map(|id| self.e_nodes.index_of_key(id).unwrap())
            }));

        // Add terminal nodes
        if terminals {
            self.e_node_terminal_idx.extend(
                self.e_nodes
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, e)| e.terminal.then_some(idx)),
            );
        }

        // Add nodes with event triggers
        if event_triggers {
            self.e_node_terminal_idx.extend(
                self.e_nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.events.iter().any(|ev| !ev.subscribers.is_empty()))
                    .map(|(idx, _)| idx),
            );
        }
    }

    fn walk_backward_collect_order(&mut self) -> Result<()> {
        self.e_node_process_order.clear();
        self.stack.clear();

        for e_node_idx in self.e_node_terminal_idx.iter().copied() {
            self.stack.push(Visit {
                e_node_idx,
                cause: VisitCause::Terminal,
            });
        }

        while let Some(visit) = self.stack.pop() {
            match visit.cause {
                VisitCause::Terminal => {}
                VisitCause::OutputRequest { output_idx } => {
                    self.e_nodes[visit.e_node_idx].outputs[output_idx].usage_count += 1;
                }
                VisitCause::Done => {
                    let e_node = &mut self.e_nodes[visit.e_node_idx];
                    assert_eq!(e_node.process_state, ProcessState::Processing);
                    e_node.process_state = ProcessState::Backward;
                    self.e_node_process_order.push(visit.e_node_idx);
                    continue;
                }
            }

            let e_node = &mut self.e_nodes[visit.e_node_idx];
            match e_node.process_state {
                ProcessState::None => unreachable!("should be Forward"),
                ProcessState::Processing => {
                    return Err(Error::CycleDetected { node_id: e_node.id });
                }
                ProcessState::Backward => continue,
                ProcessState::Forward => {}
            }

            e_node.process_state = ProcessState::Processing;
            self.stack.push(Visit {
                e_node_idx: visit.e_node_idx,
                cause: VisitCause::Done,
            });

            for e_input in e_node.inputs.iter() {
                if let ExecutionBinding::Bind(addr) = &e_input.binding {
                    self.stack.push(Visit {
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
        for e_node_idx in self.e_node_process_order.iter().copied() {
            let e_node = &self.e_nodes[e_node_idx];
            assert!(!matches!(
                e_node.process_state,
                ProcessState::None | ProcessState::Processing
            ));

            let mut inputs_updated = false;
            let mut bindings_changed = false;
            let mut missing_required = false;

            for input_idx in 0..e_node.inputs.len() {
                let e_input = &self.e_nodes[e_node_idx].inputs[input_idx];
                let (dep_wants_execute, missing) = match &e_input.binding {
                    ExecutionBinding::Undefined => unreachable!("uninitialized binding"),
                    ExecutionBinding::None => (false, e_input.required),
                    ExecutionBinding::Const(_) => (false, false),
                    ExecutionBinding::Bind(addr) => {
                        let output_node = &self.e_nodes[addr.target_idx];
                        assert_eq!(output_node.process_state, ProcessState::Forward);
                        assert!(addr.port_idx < output_node.outputs.len());
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
            e_node.process_state = ProcessState::Forward;
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
        }
    }

    fn walk_backward_collect_execute_order(&mut self) {
        self.e_node_execute_order.clear();
        self.stack.clear();

        for e_node_idx in self.e_node_terminal_idx.drain() {
            self.stack.push(Visit {
                e_node_idx,
                cause: VisitCause::Terminal,
            });
        }

        while let Some(visit) = self.stack.pop() {
            let e_node = &mut self.e_nodes[visit.e_node_idx];

            match visit.cause {
                VisitCause::Terminal | VisitCause::OutputRequest { .. } => {}
                VisitCause::Done => {
                    assert_eq!(e_node.process_state, ProcessState::Processing);
                    self.e_node_execute_order.push(visit.e_node_idx);
                    e_node.process_state = ProcessState::Backward;
                    continue;
                }
            }

            match e_node.process_state {
                ProcessState::Processing => {
                    unreachable!("cycle should be detected in walk_backward_collect_order")
                }
                ProcessState::None => unreachable!("should have been processed"),
                ProcessState::Backward => continue,
                ProcessState::Forward => {}
            }

            if !e_node.wants_execute {
                e_node.process_state = ProcessState::Backward;
                continue;
            }

            e_node.process_state = ProcessState::Processing;
            self.stack.push(Visit {
                e_node_idx: visit.e_node_idx,
                cause: VisitCause::Done,
            });

            let e_node = &self.e_nodes[visit.e_node_idx];
            for e_input in e_node.inputs.iter() {
                if e_input.dependency_wants_execute
                    && let Some(addr) = e_input.binding.as_bind()
                {
                    self.stack.push(Visit {
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

        let mut inputs: Vec<InvokeInput> = Vec::default();
        let mut output_usage: Vec<OutputUsage> = Vec::default();

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

            self.collect_inputs(e_node_idx, &mut inputs);
            self.collect_output_usage(e_node_idx, &mut output_usage);

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
                    &inputs,
                    &output_usage,
                    outputs,
                )
                .await
                .map_err(|e| Error::Invoke {
                    func_id: e_node.func_id,
                    message: e.to_string(),
                });

            e_node.run_time = start.elapsed().as_secs_f64();

            for e_input in &mut e_node.inputs {
                assert!(!matches!(e_input.binding, ExecutionBinding::Undefined));
                e_input.binding_changed = false;
            }

            if let Err(err) = result {
                e_node.error = Some(err);
                e_node.output_values = None;
            }
        }

        self.collect_execution_stats(start)
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
                ExecutionBinding::Undefined => unreachable!("uninitialized binding"),
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
                        missing_inputs.push(PortAddress {
                            target_id: e.id,
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
                ExecutionBinding::Undefined | ExecutionBinding::None => None,
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

    // === Validation ===

    fn validate_with(&self, graph: &Graph, func_lib: &FuncLib) {
        if !is_debug() {
            return;
        }

        assert!(self.e_nodes.len() == graph.nodes.len());
        assert!(self.e_node_process_order.len() <= self.e_nodes.len());

        let mut seen_node_ids: HashSet<NodeId> = HashSet::with_capacity(self.e_nodes.len());
        for e_node in self.e_nodes.iter() {
            assert!(seen_node_ids.insert(e_node.id));

            if let Some(output_values) = e_node.output_values.as_ref() {
                assert_eq!(output_values.len(), e_node.outputs.len());
            }

            assert_ne!(e_node.process_state, ProcessState::Processing);
            assert_ne!(e_node.process_state, ProcessState::None);

            let node = graph.by_id(&e_node.id).unwrap();
            let func = func_lib.by_id(&e_node.func_id).unwrap();

            assert_eq!(e_node.func_id, node.func_id);
            assert_eq!(node.id, e_node.id);
            assert_eq!(node.func_id, func.id);
            assert_eq!(e_node.inputs.len(), node.inputs.len());
            assert_eq!(node.inputs.len(), func.inputs.len());
            assert_eq!(e_node.outputs.len(), func.outputs.len());

            for (input, e_input) in node.inputs.iter().zip(e_node.inputs.iter()) {
                match (&input.binding, &e_input.binding) {
                    (Binding::None, ExecutionBinding::None) => {}
                    (Binding::Const(v1), ExecutionBinding::Const(v2)) => assert_eq!(v1, v2),
                    (Binding::Bind(addr), ExecutionBinding::Bind(e_addr)) => {
                        assert!(e_addr.target_idx < self.e_nodes.len());
                        let target = &self.e_nodes[e_addr.target_idx];
                        assert!(e_addr.port_idx < target.outputs.len());
                        assert_eq!(addr.port_idx, e_addr.port_idx);
                        assert_eq!(addr.target_id, target.id);
                    }
                    _ => panic!("Mismatched bindings"),
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
                match &input.binding {
                    ExecutionBinding::Undefined => panic!("uninitialized binding"),
                    ExecutionBinding::Bind(addr) => {
                        assert!(addr.target_idx < self.e_nodes.len());
                        assert!(seen_in_order.contains(&addr.target_idx));
                    }
                    _ => {}
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
            assert!(!matches!(
                e_node.process_state,
                ProcessState::Processing | ProcessState::None
            ));

            if e_node.missing_required_inputs {
                assert!(!e_node.wants_execute);
            }

            for e_input in e_node.inputs.iter() {
                assert!(!matches!(e_input.binding, ExecutionBinding::Undefined));
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
            assert_eq!(e_node.process_state, ProcessState::Backward);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::data::{DynamicValue, StaticValue};
    use crate::function::{TestFuncHooks, test_func_lib};
    use crate::graph::{NodeBehavior, test_graph};
    use common::{FloatExt, SerdeFormat};
    use tokio::sync::Mutex;

    fn execution_node_names_in_order(execution_graph: &ExecutionGraph) -> Vec<String> {
        execution_graph
            .e_node_execute_order
            .iter()
            .map(|&e_node_idx| execution_graph.e_nodes[e_node_idx].name.clone())
            .collect()
    }

    #[test]
    fn basic_run() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "print"]
        );

        assert_eq!(execution_graph.e_nodes.len(), 5);
        assert_eq!(execution_graph.e_node_process_order.len(), 5);
        assert_eq!(execution_graph.e_node_execute_order.len(), 5);
        assert!(
            execution_graph
                .e_nodes
                .iter()
                .all(|e_node| !e_node.missing_required_inputs)
        );
        assert!(
            execution_graph
                .e_nodes
                .iter()
                .all(|e_node| e_node.wants_execute)
        );

        let get_a = execution_graph.by_name("get_a").unwrap();
        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert_eq!(get_a.outputs[0].usage_count, 1);
        assert_eq!(get_b.outputs[0].usage_count, 2);
        assert_eq!(sum.outputs[0].usage_count, 1);
        assert_eq!(mult.outputs[0].usage_count, 1);

        assert!(!get_a.inputs_updated);
        assert!(!get_b.inputs_updated);
        assert!(sum.inputs_updated);
        assert!(mult.inputs_updated);
        assert!(print.inputs_updated);

        assert!(print.terminal);

        Ok(())
    }

    #[test]
    fn missing_input() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert!(!get_b.missing_required_inputs);
        assert!(sum.missing_required_inputs);
        assert!(mult.missing_required_inputs);
        assert!(print.missing_required_inputs);

        assert_eq!(execution_graph.e_node_execute_order.len(), 0);

        Ok(())
    }

    #[test]
    fn missing_non_required_input() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;
        func_lib.by_name_mut("mult").unwrap().inputs[0].required = false;

        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert!(sum.missing_required_inputs);
        assert!(!mult.missing_required_inputs);
        assert!(!print.missing_required_inputs);

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn const_binding() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(1)),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_| {}),
        });
        let mut execution_graph = ExecutionGraph::default();

        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(3));
        mult.inputs[1].binding = Binding::Const(StaticValue::Int(5));

        execution_graph.update(&graph, &func_lib);

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(mult.inputs[0].binding_changed);
        assert!(mult.inputs[1].binding_changed);

        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(mult.inputs_updated);
        assert!(!mult.inputs[0].binding_changed);
        assert!(!mult.inputs[0].dependency_wants_execute);
        assert!(!mult.inputs[1].binding_changed);
        assert!(!mult.inputs[1].dependency_wants_execute);

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(!mult.inputs_updated);
        assert!(!mult.inputs[0].binding_changed);
        assert!(!mult.inputs[1].binding_changed);

        graph.by_name_mut("mult").unwrap().inputs[0].binding = Binding::Const(StaticValue::Int(4));
        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        assert!(mult.inputs[0].binding_changed);
        assert!(!mult.inputs[1].binding_changed);
        assert!(!mult.missing_required_inputs);
        assert!(!print.missing_required_inputs);
        assert!(mult.inputs_updated);
        assert!(print.inputs_updated);

        Ok(())
    }

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

        for format in SerdeFormat::all_formats_for_testing() {
            let serialized = execution_graph.serialize(format);
            let deserialized = ExecutionGraph::deserialize(&serialized, format)?;
            let serialized_again = deserialized.serialize(format);
            assert_eq!(serialized, serialized_again);
        }

        Ok(())
    }

    #[test]
    fn execution_graph_updates_after_graph_change() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);

        let binding1: Binding = (graph.by_name("get_a").unwrap().id, 0).into();
        let binding2: Binding = (graph.by_name("get_b").unwrap().id, 0).into();
        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = binding1;
        mult.inputs[1].binding = binding2;

        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        let get_a = execution_graph.by_name("get_a").unwrap();
        let get_b = execution_graph.by_name("get_b").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert_eq!(get_a.outputs.len(), 1);
        assert_eq!(get_b.outputs.len(), 1);
        assert_eq!(mult.outputs.len(), 1);
        assert!(print.outputs.is_empty());
        assert_eq!(get_a.outputs[0].usage_count, 1);
        assert_eq!(get_b.outputs[0].usage_count, 1);
        assert_eq!(mult.outputs[0].usage_count, 1);

        Ok(())
    }

    #[test]
    fn pure_node_skips_consequent_invokations() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());

        graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::AsFunction;
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Pure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        assert!(execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        assert!(!execution_node_names_in_order(&execution_graph).contains(&"get_b".to_string()));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn node_skips_consequent_invokations() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(1)),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_| {}),
        });
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "print"]
        );

        let exe_stats = execution_graph.execute_terminals().await?;
        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);
        assert_eq!(exe_stats.cached_nodes.len(), 4);

        Ok(())
    }

    #[test]
    fn inpure_node_always_invoked() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());

        graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::AsFunction;
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);
        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "print"]
        );

        Ok(())
    }

    #[test]
    fn once_node_always_caches() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::Once;
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "print"]
        );

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);
        execution_graph.update(&graph, &func_lib);
        execution_graph.prepare_execution(true, false, &[])?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_a", "sum", "mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn once_node_recomputes_on_binding_change() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(3)),
            get_b: Arc::new(move || 55),
            print: Arc::new(move |_| {}),
        });
        let mut execution_graph = ExecutionGraph::default();

        graph.by_name_mut("mult").unwrap().behavior = NodeBehavior::Once;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "print"]
        );

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = mult.inputs[1].binding.clone();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn once_node_recomputes_on_binding_change_with_cached_inputs() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(3)),
            get_b: Arc::new(move || 55),
            print: Arc::new(move |_| {}),
        });
        let mut execution_graph = ExecutionGraph::default();

        graph.by_name_mut("mult").unwrap().behavior = NodeBehavior::Once;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph)[2..],
            ["sum", "mult", "print"]
        );

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        let mult = graph.by_name_mut("mult").unwrap();
        let old_binding0 = mult.inputs[0].binding.clone();
        let old_binding1 = mult.inputs[1].binding.clone();
        mult.inputs[0].binding = Binding::Const(2.into());
        mult.inputs[1].binding = Binding::Const(22.into());

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = old_binding1;
        mult.inputs[1].binding = old_binding0;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"],
        );

        Ok(())
    }

    #[test]
    fn cycle_detection_returns_error() {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mult_node_id = graph.by_name("mult").unwrap().id;
        let sum_inputs = &mut graph.by_name_mut("sum").unwrap().inputs;
        sum_inputs[0].binding = (mult_node_id, 0).into();

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

        let err = execution_graph
            .prepare_execution(true, false, &[])
            .expect_err("Expected cycle detection error");
        match err {
            Error::CycleDetected { node_id } => {
                assert_eq!(node_id, "579ae1d6-10a3-4906-8948-135cb7d7508b".into());
            }
            _ => panic!("Unexpected error"),
        }
    }

    #[test]
    fn invalidate_recursively_marks_dependents() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

        let sum = graph.by_name("sum").unwrap().id;

        execution_graph.invalidate_recursively(vec![sum]);

        Ok(())
    }

    #[derive(Debug)]
    struct TestValues {
        a: i64,
        b: i64,
        result: i64,
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn simple_compute_test() -> anyhow::Result<()> {
        let test_values = Arc::new(Mutex::new(TestValues {
            a: 2,
            b: 5,
            result: 0,
        }));

        let test_values_a = test_values.clone();
        let test_values_b = test_values.clone();
        let test_values_result = test_values.clone();
        let mut func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(test_values_a.try_lock().unwrap().a)),
            get_b: Arc::new(move || test_values_b.try_lock().unwrap().b),
            print: Arc::new(move |result| {
                test_values_result.try_lock().unwrap().result = result;
            }),
        });

        let graph = test_graph();

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;
        assert_eq!(test_values.try_lock()?.result, 35);

        test_values.try_lock()?.b = 7;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;
        assert_eq!(test_values.try_lock()?.result, 35);

        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(test_values.try_lock()?.result, 63);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn const_binding_invokes_only_once() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || unreachable!()),
            get_b: Arc::new(move || unreachable!()),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(3));
        mult.inputs[1].binding = Binding::Const(StaticValue::Int(5));

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        graph.by_name_mut("mult").unwrap().inputs[0].binding = Binding::Const(StaticValue::Int(3));
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(4));
        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn optional_input_binding_change_recomputes() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(1)),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        let sum = graph.by_name_mut("mult").unwrap();
        sum.inputs[0].binding = Binding::Const(2.into());
        sum.inputs[1].binding = Binding::None;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn const_input_excludes_upstream_node() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(1)),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::Const(33.into());

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "sum", "mult", "print"]
        );

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[1].binding = Binding::None;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["sum", "mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn change_from_const_to_bind_recomputes() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(1)),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        let get_b_id = graph.by_name_mut("get_b").unwrap().id;
        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::Const(33.into());

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["get_b", "sum", "mult", "print"]
        );

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = (get_b_id, 0).into();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["sum", "mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn required_none_binding_execute_is_stable() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(1)),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::None;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;
        execution_graph.execute_terminals().await?;

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_upstream_output_reused_after_rebinding() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(1)),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(2.into());
        mult.inputs[1].binding = Binding::Const(21.into());

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        let get_b_id = graph.by_name_mut("get_b").unwrap().id;
        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = (get_b_id, 0).into();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn once_node_toggle_refreshes_upstream_execution() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(1)),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::Const(2.into());
        sum.inputs[1].binding = Binding::Const(21.into());

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::Const(12.into());
        let mult = graph.by_name_mut("mult").unwrap();
        mult.behavior = NodeBehavior::Once;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        let sum = execution_graph.by_name("sum").unwrap();
        assert!(!sum.cached);

        assert_eq!(execution_node_names_in_order(&execution_graph), ["print"]);

        let mult = graph.by_name_mut("mult").unwrap();
        mult.behavior = NodeBehavior::AsFunction;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        assert_eq!(
            execution_node_names_in_order(&execution_graph),
            ["sum", "mult", "print"]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_input_output_values_returns_none_for_nonexistent_node() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);

        let nonexistent_id: NodeId = "00000000-0000-0000-0000-000000000000".into();
        assert!(
            execution_graph
                .get_argument_values(&nonexistent_id)
                .is_none()
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_input_output_values_with_const_bindings() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || unreachable!()),
            get_b: Arc::new(move || unreachable!()),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(3));
        mult.inputs[1].binding = Binding::Const(StaticValue::Int(5));
        let mult_id = mult.id;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        let values = execution_graph.get_argument_values(&mult_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(matches!(values.inputs[0], Some(DynamicValue::Int(3))));
        assert!(matches!(values.inputs[1], Some(DynamicValue::Int(5))));

        assert_eq!(values.outputs.len(), 1);
        assert!(matches!(values.outputs[0], DynamicValue::Int(15)));

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_input_output_values_with_bound_outputs() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(2)),
            get_b: Arc::new(move || 5),
            print: Arc::new(move |_| {}),
        });

        let graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        let sum_id = graph.by_name("sum").unwrap().id;
        let values = execution_graph.get_argument_values(&sum_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(
            matches!(values.inputs[0], Some(DynamicValue::Float(v)) if v.approximately_eq(2.0))
        );
        assert!(
            matches!(values.inputs[1], Some(DynamicValue::Float(v)) if v.approximately_eq(5.0))
        );

        assert_eq!(values.outputs.len(), 1);
        assert!(matches!(values.outputs[0], DynamicValue::Int(7)));

        let mult_id = graph.by_name("mult").unwrap().id;
        let values = execution_graph.get_argument_values(&mult_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(matches!(values.inputs[0], Some(DynamicValue::Int(7))));
        assert!(
            matches!(values.inputs[1], Some(DynamicValue::Float(v)) if v.approximately_eq(5.0))
        );

        assert_eq!(values.outputs.len(), 1);
        assert!(matches!(values.outputs[0], DynamicValue::Int(35)));

        let print_id = graph.by_name("print").unwrap().id;
        let values = execution_graph.get_argument_values(&print_id).unwrap();

        assert_eq!(values.inputs.len(), 1);
        assert!(matches!(values.inputs[0], Some(DynamicValue::Int(35))));
        assert!(values.outputs.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_input_output_values_with_none_binding() -> anyhow::Result<()> {
        let mut func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || Ok(1)),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        func_lib.by_name_mut("mult").unwrap().inputs[1].required = false;
        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[1].binding = Binding::None;
        let mult_id = mult.id;

        execution_graph.update(&graph, &func_lib);
        execution_graph.execute_terminals().await?;

        let values = execution_graph.get_argument_values(&mult_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(values.inputs[0].is_some());
        assert!(values.inputs[1].is_none());

        Ok(())
    }

    #[test]
    fn get_input_output_values_before_execution() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib);

        let sum_id = graph.by_name("sum").unwrap().id;
        let values = execution_graph.get_argument_values(&sum_id).unwrap();

        assert_eq!(values.inputs.len(), 2);
        assert!(values.inputs[0].is_none());
        assert!(values.inputs[1].is_none());
        assert!(values.outputs.is_empty());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn node_error_propagates_to_dependents() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(|| Err(anyhow::anyhow!("Intentional failure in get_a"))),
            get_b: Arc::new(|| 42),
            print: Arc::new(|_| {}),
        });

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

        let stats = execution_graph.execute_terminals().await?;

        let get_a = execution_graph.by_name("get_a").unwrap();
        assert!(get_a.error.is_some());
        assert!(get_a.output_values.is_none());

        let get_b = execution_graph.by_name("get_b").unwrap();
        assert!(get_b.error.is_none());
        assert!(get_b.output_values.is_some());
        assert!(
            get_b.output_values.as_ref().unwrap()[0]
                .as_f64()
                .unwrap()
                .approximately_eq(42.0)
        );

        let sum = execution_graph.by_name("sum").unwrap();
        assert!(sum.error.is_some());
        assert!(sum.output_values.is_none());
        assert!(
            sum.error
                .as_ref()
                .unwrap()
                .to_string()
                .contains("upstream error")
        );

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(mult.error.is_some());
        assert!(mult.output_values.is_none());
        assert!(
            mult.error
                .as_ref()
                .unwrap()
                .to_string()
                .contains("upstream error")
        );

        let print = execution_graph.by_name("print").unwrap();
        assert!(print.error.is_some());
        assert!(print.output_values.is_none());

        assert_eq!(stats.node_errors.len(), 4);

        let get_a_error = stats
            .node_errors
            .iter()
            .find(|e| e.node_id == get_a.id)
            .unwrap();
        assert!(
            get_a_error
                .error
                .to_string()
                .contains("Intentional failure")
        );

        let sum_error = stats
            .node_errors
            .iter()
            .find(|e| e.node_id == sum.id)
            .unwrap();
        assert!(sum_error.error.to_string().contains("upstream error"));

        Ok(())
    }
}
