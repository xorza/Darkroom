use std::mem::take;
use std::panic;

use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use hashbrown::HashSet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::context::ContextManager;
use crate::data::{DataType, DynamicValue, StaticValue};
use crate::function::{Func, FuncBehavior, FuncLib, InvokeCache, InvokeInput};
use crate::graph::{Binding, Graph, Node, NodeBehavior, NodeId};
use crate::prelude::{FuncId, FuncLambda};
use common::{is_debug, BoolExt, FileFormat};

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum ExecutionError {
    #[error("Function invocation failed for function {func_id:?}: {message}")]
    Invoke { func_id: FuncId, message: String },
    #[error("Cycle detected while building execution graph at node {node_id:?}")]
    CycleDetected { node_id: NodeId },
}

pub type ExecutionResult<T> = std::result::Result<T, ExecutionError>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExecutionStats {
    pub elapsed_secs: f64,
    pub executed_nodes: usize,
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionInput {
    pub state: InputState,
    pub required: bool,
    pub binding: ExecutionBinding,
    pub data_type: DataType,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ExecutionOutput {
    usage_count: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExecutionBehavior {
    #[default]
    Impure,
    Pure,
    Once,
}
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
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionPortAddress {
    pub target_idx: usize,
    pub port_idx: usize,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionBinding {
    #[default]
    Undefined,
    None,
    Const(StaticValue),
    Bind(ExecutionPortAddress),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum ProcessState {
    #[default]
    None,
    Processing,
    Backward,
    Forward,
}
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: NodeId,
    inited: bool,

    // used only for debuggind and assertions
    process_state: ProcessState,

    pub terminal: bool,
    pub missing_required_inputs: bool,
    pub wants_execute: bool,
    pub changed_inputs: bool,
    pub behavior: ExecutionBehavior,

    pub inputs: Vec<ExecutionInput>,
    pub outputs: Vec<ExecutionOutput>,

    pub func_id: FuncId,

    pub run_time: f64,
    pub error: Option<ExecutionError>,

    #[serde(skip)]
    pub(crate) cache: InvokeCache,
    #[serde(skip)]
    pub(crate) output_values: Option<Vec<DynamicValue>>,

    #[serde(skip)]
    pub lambda: FuncLambda,

    #[cfg(debug_assertions)]
    pub name: String,
}
impl KeyIndexKey<NodeId> for ExecutionNode {
    fn key(&self) -> &NodeId {
        &self.id
    }
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub e_nodes: KeyIndexVec<NodeId, ExecutionNode>,
    pub e_node_process_order: Vec<usize>,
    pub e_node_invoke_order: Vec<usize>,

    #[serde(skip)]
    ctx_manager: ContextManager,

    //caches
    #[serde(skip)]
    stack: Vec<Visit>,
}

impl ExecutionBinding {
    pub fn as_const(&self) -> Option<&StaticValue> {
        match self {
            ExecutionBinding::Const(static_value) => Some(static_value),
            _ => None,
        }
    }
    pub fn as_bind(&self) -> Option<&ExecutionPortAddress> {
        match self {
            ExecutionBinding::Bind(port_address) => Some(port_address),
            _ => None,
        }
    }
    pub fn is_none(&self) -> bool {
        matches!(self, ExecutionBinding::None)
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
        self.changed_inputs = false;
        self.missing_required_inputs = false;
        self.run_time = 0.0;
        self.error = None;
    }
    fn reset(&mut self, node: &Node, func: &Func) {
        assert_eq!(self.id, node.id);
        assert_eq!(node.func_id, func.id);
        assert_eq!(node.inputs.len(), func.inputs.len());

        if !self.inited {
            // assume that functions never change in runtime
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
        }

        self.terminal = node.terminal;
        self.process_state = ProcessState::None;

        self.behavior = match node.behavior {
            NodeBehavior::AsFunction => match func.behavior {
                FuncBehavior::Pure => ExecutionBehavior::Pure,
                FuncBehavior::Impure => ExecutionBehavior::Impure,
            },
            NodeBehavior::Once => ExecutionBehavior::Once,
        };

        self.outputs.clear();
        self.outputs
            .resize(func.outputs.len(), ExecutionOutput::default());

        assert_eq!(self.inputs.len(), node.inputs.len());
        assert_eq!(self.outputs.len(), func.outputs.len());

        #[cfg(debug_assertions)]
        {
            self.name.clear();
            self.name.push_str(&node.name);
        }
    }
}

impl ExecutionGraph {
    pub fn by_id(&self, node_id: &NodeId) -> Option<&ExecutionNode> {
        self.e_nodes.by_key(node_id)
    }
    pub fn by_id_mut(&mut self, node_id: &NodeId) -> Option<&mut ExecutionNode> {
        self.e_nodes.by_key_mut(node_id)
    }

    pub fn invalidate_recursively<I>(&mut self, node_ids: I)
    where
        I: IntoIterator<Item = NodeId>,
    {
        let mut stack: Vec<usize> = node_ids
            .into_iter()
            .filter_map(|id| self.e_nodes.index_of_key(&id))
            .collect();
        let mut seen: Vec<bool> = vec![false; self.e_nodes.len()];

        while let Some(e_node_idx) = stack.pop() {
            if seen[e_node_idx] {
                continue;
            }
            seen[e_node_idx] = true;

            self.e_nodes[e_node_idx].invalidate();

            for (output_e_node_idx, e_node) in self.e_nodes.iter().enumerate() {
                if seen[output_e_node_idx] {
                    continue;
                }

                let depends = e_node.inputs.iter().any(|input| {
                    matches!(
                        &input.binding,
                        ExecutionBinding::Bind(port_address)
                            if port_address.target_idx == e_node_idx
                    )
                });

                if depends {
                    stack.push(output_e_node_idx);
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.e_nodes.clear();
        self.e_node_invoke_order.clear();
    }

    #[cfg(debug_assertions)]
    pub fn by_name(&self, node_name: &str) -> Option<&ExecutionNode> {
        self.e_nodes.iter().find(|node| node.name == node_name)
    }
    #[cfg(debug_assertions)]
    pub fn by_name_mut(&mut self, node_name: &str) -> Option<&mut ExecutionNode> {
        self.e_nodes.iter_mut().find(|node| node.name == node_name)
    }

    pub fn serialize(&self, format: FileFormat) -> String {
        common::serialize(self, format)
    }
    pub fn deserialize(serialized: &str, format: FileFormat) -> anyhow::Result<Self> {
        Ok(common::deserialize(serialized, format)?)
    }

    // Walk upstream dependencies to collect active nodes in processing order for input-state evaluation.
    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) -> ExecutionResult<()> {
        graph.validate_with(func_lib);

        self.e_node_invoke_order.clear();
        self.e_node_process_order.clear();
        self.e_node_process_order.reserve(graph.nodes.len());

        self.forward1(graph, func_lib);
        self.backward1()?;

        self.validate_with(graph, func_lib);

        Ok(())
    }

    fn forward1(&mut self, graph: &Graph, func_lib: &FuncLib) {
        self.e_nodes
            .iter_mut()
            .for_each(|e_node| e_node.process_state = ProcessState::None);

        let mut write_idx = 0;

        for node in graph.nodes.iter() {
            let e_node_idx = self
                .e_nodes
                .compact_insert_with(&node.id, &mut write_idx, || ExecutionNode {
                    id: node.id,
                    ..Default::default()
                });

            let e_node = &mut self.e_nodes[e_node_idx];
            assert_eq!(e_node.process_state, ProcessState::None);
            let node = graph.by_id(&e_node.id).unwrap();
            let func = func_lib.by_id(&node.func_id).unwrap();
            e_node.reset(node, func);
            e_node.process_state = ProcessState::Forward;

            for (input_idx, input) in node.inputs.iter().enumerate() {
                let e_input = &mut self.e_nodes[e_node_idx].inputs[input_idx];
                e_input.state = match (&input.binding, &e_input.binding) {
                    (Binding::None, ExecutionBinding::None) => InputState::Unchanged,
                    (Binding::None, _) => {
                        e_input.binding = ExecutionBinding::None;
                        InputState::Changed
                    }
                    (Binding::Const(value), ExecutionBinding::Const(existing))
                        if value == existing =>
                    {
                        InputState::Unchanged
                    }
                    (Binding::Const(value), _) => {
                        e_input.binding = ExecutionBinding::Const(value.clone());
                        InputState::Changed
                    }
                    (Binding::Bind(_), ExecutionBinding::Bind(_)) => {
                        e_input.binding = ExecutionBinding::Undefined;
                        InputState::Unchanged
                    }
                    (Binding::Bind(_), _) => {
                        e_input.binding = ExecutionBinding::Undefined;
                        InputState::Changed
                    }
                };
                let Binding::Bind(port_address) = &input.binding else {
                    continue;
                };
                let output_e_node_idx = self.e_nodes.compact_insert_with(
                    &port_address.target_id,
                    &mut write_idx,
                    || ExecutionNode {
                        id: port_address.target_id,
                        ..Default::default()
                    },
                );

                let e_input = &mut self.e_nodes[e_node_idx].inputs[input_idx];
                e_input.binding = ExecutionBinding::Bind(ExecutionPortAddress {
                    target_idx: output_e_node_idx,
                    port_idx: port_address.port_idx,
                });

                assert_ne!(e_input.binding, ExecutionBinding::Undefined);
            }
        }

        self.e_nodes.compact_finish(write_idx);
    }

    fn backward1(&mut self) -> ExecutionResult<()> {
        let mut stack: Vec<Visit> = take(&mut self.stack);
        stack.clear();

        for (e_node_idx, e_node) in self.e_nodes.iter().enumerate() {
            if e_node.terminal {
                stack.push(Visit {
                    e_node_idx,
                    cause: VisitCause::Terminal,
                });
            }
        }

        while let Some(visit) = stack.pop() {
            match visit.cause {
                VisitCause::Terminal => {}
                VisitCause::OutputRequest { output_idx } => {
                    let e_node = &mut self.e_nodes[visit.e_node_idx];
                    e_node.outputs[output_idx].usage_count += 1;
                }
                VisitCause::Done => {
                    let e_node = &mut self.e_nodes[visit.e_node_idx];
                    assert_eq!(e_node.process_state, ProcessState::Processing);
                    e_node.process_state = ProcessState::Backward;
                    self.e_node_process_order.push(visit.e_node_idx);
                    continue;
                }
            };

            let e_node = &self.e_nodes[visit.e_node_idx];
            match e_node.process_state {
                ProcessState::None => unreachable!("should be Forward"),
                ProcessState::Processing => {
                    return Err(ExecutionError::CycleDetected { node_id: e_node.id });
                }
                ProcessState::Backward => continue,
                ProcessState::Forward => {}
            }

            let e_node = &mut self.e_nodes[visit.e_node_idx];
            e_node.process_state = ProcessState::Processing;
            stack.push(Visit {
                e_node_idx: visit.e_node_idx,
                cause: VisitCause::Done,
            });

            for e_input in e_node.inputs.iter() {
                if let ExecutionBinding::Bind(port_address) = &e_input.binding {
                    stack.push(Visit {
                        e_node_idx: port_address.target_idx,
                        cause: VisitCause::OutputRequest {
                            output_idx: port_address.port_idx,
                        },
                    });
                };
            }
        }

        Ok(())
    }

    pub async fn execute(&mut self) -> ExecutionResult<ExecutionStats> {
        self.pre_execute();

        let start = std::time::Instant::now();

        let mut inputs: Vec<InvokeInput> = Vec::default();
        let mut output_usage: Vec<OutputUsage> = Vec::default();
        let mut error: Option<ExecutionError> = None;

        for e_node_idx in self.e_node_invoke_order.iter().copied() {
            let e_node = &self.e_nodes[e_node_idx];

            inputs.clear();
            for input in e_node.inputs.iter() {
                let value: DynamicValue = match &input.binding {
                    ExecutionBinding::Undefined => unreachable!("uninitialized binding"),
                    ExecutionBinding::None => DynamicValue::None,
                    ExecutionBinding::Const(value) => value.into(),
                    ExecutionBinding::Bind(port_address) => {
                        let output_e_node = &self.e_nodes[port_address.target_idx];
                        let output_values = output_e_node
                            .output_values
                            .as_ref()
                            .expect("Output values missing for bound node; check execution order");

                        assert_eq!(output_values.len(), output_e_node.outputs.len());
                        output_values[port_address.port_idx].clone()
                    }
                };

                let value = value.convert_type(&input.data_type);

                inputs.push(InvokeInput {
                    state: input.state,
                    value,
                });
            }

            output_usage.clear();
            output_usage.extend(e_node.outputs.iter().map(|output| {
                (output.usage_count == 0).then_else(OutputUsage::Skip, OutputUsage::Needed)
            }));

            let e_node = &mut self.e_nodes[e_node_idx];
            assert!(e_node.error.is_none());

            let outputs = e_node
                .output_values
                .get_or_insert_with(|| vec![DynamicValue::None; e_node.outputs.len()]);

            let start = std::time::Instant::now();
            let invoke_result = e_node
                .lambda
                .invoke(
                    &mut self.ctx_manager,
                    &mut e_node.cache,
                    inputs.as_slice(),
                    output_usage.as_slice(),
                    outputs.as_mut_slice(),
                )
                .await
                .map_err(|source| ExecutionError::Invoke {
                    func_id: e_node.func_id,
                    message: source.to_string(),
                });

            e_node.run_time = start.elapsed().as_secs_f64();

            // after node execution assume unchanged
            e_node.inputs.iter_mut().for_each(|input| {
                assert_ne!(input.binding, ExecutionBinding::Undefined);
                input.state = InputState::Unchanged;
            });

            if let Err(err) = invoke_result {
                e_node.error = Some(err.clone());
                error = Some(err);
                break;
            }
        }

        match error {
            Some(err) => Err(err),
            None => Ok(ExecutionStats {
                elapsed_secs: start.elapsed().as_secs_f64(),
                executed_nodes: self.e_node_invoke_order.len(),
            }),
        }
    }

    fn pre_execute(&mut self) {
        self.e_nodes.iter_mut().for_each(|e_node| {
            e_node.reset_for_execution();
        });

        self.forward2();
        self.backward2();

        self.validate_for_execution();
    }

    // Propagate input state forward through the processing order.
    fn forward2(&mut self) {
        for e_node_idx in self.e_node_process_order.iter().copied() {
            let e_node = &mut self.e_nodes[e_node_idx];

            let mut changed_inputs = false;
            let mut missing_required_inputs = false;

            assert_ne!(e_node.process_state, ProcessState::None);
            assert_ne!(e_node.process_state, ProcessState::Processing);

            for input_idx in 0..self.e_nodes[e_node_idx].inputs.len() {
                let e_input = &self.e_nodes[e_node_idx].inputs[input_idx];
                match &e_input.binding {
                    ExecutionBinding::Undefined => unreachable!("uninitialized binding"),
                    ExecutionBinding::None => missing_required_inputs |= e_input.required,
                    ExecutionBinding::Const(_) => {}
                    ExecutionBinding::Bind(port_address) => {
                        let output_e_node = &self.e_nodes[port_address.target_idx];

                        assert_eq!(output_e_node.process_state, ProcessState::Forward);
                        assert!(port_address.port_idx < output_e_node.outputs.len());

                        missing_required_inputs |=
                            e_input.required && output_e_node.missing_required_inputs;
                        let output_e_node_wants_execute = output_e_node.wants_execute;

                        let e_input = &mut self.e_nodes[e_node_idx].inputs[input_idx];
                        e_input.state = output_e_node_wants_execute
                            .then_else(InputState::Changed, e_input.state);
                    }
                };

                let e_input = &self.e_nodes[e_node_idx].inputs[input_idx];
                changed_inputs |= e_input.state == InputState::Changed;
            }

            let e_node = &mut self.e_nodes[e_node_idx];
            e_node.process_state = ProcessState::Forward;
            e_node.changed_inputs = changed_inputs;
            e_node.missing_required_inputs = missing_required_inputs;
            e_node.wants_execute = !e_node.missing_required_inputs
                && match e_node.behavior {
                    ExecutionBehavior::Impure => true,
                    ExecutionBehavior::Pure => {
                        e_node.output_values.is_none() || e_node.changed_inputs
                    }
                    ExecutionBehavior::Once => e_node.output_values.is_none(),
                };
        }
    }

    // Walk upstream dependencies to collect the execution order.
    fn backward2(&mut self) {
        self.e_node_invoke_order.clear();
        self.e_node_invoke_order
            .reserve(self.e_node_process_order.len());

        let mut stack: Vec<Visit> = take(&mut self.stack);
        assert!(stack.is_empty());

        for (e_node_idx, e_node) in self.e_nodes.iter().enumerate() {
            if e_node.terminal {
                stack.push(Visit {
                    e_node_idx,
                    cause: VisitCause::Terminal,
                });
            }
        }

        while let Some(visit) = stack.pop() {
            let e_node = &mut self.e_nodes[visit.e_node_idx];

            match visit.cause {
                VisitCause::Terminal | VisitCause::OutputRequest { .. } => {}
                VisitCause::Done => {
                    assert_eq!(e_node.process_state, ProcessState::Processing);
                    self.e_node_invoke_order.push(visit.e_node_idx);
                    e_node.process_state = ProcessState::Backward;
                    continue;
                }
            };

            match e_node.process_state {
                ProcessState::Processing => {
                    unreachable!("Cycles should be detected earlier in backward1()")
                }
                ProcessState::None => unreachable!("Node should have been processed in forward()"),
                ProcessState::Forward => {}
                ProcessState::Backward => continue,
            }

            if !e_node.wants_execute {
                e_node.process_state = ProcessState::Backward;
                continue;
            }

            e_node.process_state = ProcessState::Processing;
            stack.push(Visit {
                e_node_idx: visit.e_node_idx,
                cause: VisitCause::Done,
            });

            let e_node = &self.e_nodes[visit.e_node_idx];
            for e_input in e_node.inputs.iter() {
                if e_input.state != InputState::Changed {
                    continue;
                }

                let Some(port_address) = e_input.binding.as_bind() else {
                    continue;
                };

                stack.push(Visit {
                    e_node_idx: port_address.target_idx,
                    cause: VisitCause::OutputRequest {
                        output_idx: port_address.port_idx,
                    },
                });
            }
        }

        self.stack = take(&mut stack);
    }

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
                let binding = &input.binding;
                let e_binding = &e_input.binding;
                match (binding, e_binding) {
                    (Binding::None, ExecutionBinding::None) => {}
                    (Binding::Const(static_value1), ExecutionBinding::Const(static_value2)) => {
                        assert_eq!(static_value1, static_value2);
                    }
                    (
                        Binding::Bind(port_address),
                        ExecutionBinding::Bind(execution_port_address),
                    ) => {
                        assert!(execution_port_address.target_idx < self.e_nodes.len());
                        let target_e_node = &self.e_nodes[execution_port_address.target_idx];
                        assert!(execution_port_address.port_idx < target_e_node.outputs.len());
                        assert_eq!(port_address.port_idx, execution_port_address.port_idx);
                        assert_eq!(port_address.target_id, target_e_node.id);
                    }
                    (_, _) => panic!("Mismatched bindings"),
                }
            }
        }

        let mut seen = HashSet::with_capacity(self.e_nodes.len());
        for &e_node_idx in self.e_node_process_order.iter() {
            assert!(e_node_idx < self.e_nodes.len());
            assert!(seen.insert(e_node_idx));
        }

        let mut seen_in_process_order = HashSet::with_capacity(self.e_nodes.len());
        for &e_node_idx in self.e_node_process_order.iter() {
            assert!(e_node_idx < self.e_nodes.len());

            let e_node = &self.e_nodes[e_node_idx];
            for input in e_node.inputs.iter() {
                match &input.binding {
                    ExecutionBinding::Undefined => panic!("uninitialized binding"),
                    ExecutionBinding::Bind(port_address) => {
                        assert!(port_address.target_idx < self.e_nodes.len());
                        assert!(seen_in_process_order.contains(&port_address.target_idx));
                    }
                    ExecutionBinding::None | ExecutionBinding::Const(_) => {}
                }
            }

            assert!(seen_in_process_order.insert(e_node_idx));
        }
    }

    fn validate_for_execution(&self) {
        if !is_debug() {
            return;
        }

        for e_node in self.e_nodes.iter() {
            assert_ne!(e_node.process_state, ProcessState::Processing);
            assert_ne!(e_node.process_state, ProcessState::None);

            if e_node.missing_required_inputs {
                assert!(!e_node.wants_execute);
            }

            for e_input in e_node.inputs.iter() {
                assert_ne!(e_input.binding, ExecutionBinding::Undefined);

                if let ExecutionBinding::Bind(port_address) = &e_input.binding {
                    assert!(port_address.target_idx < self.e_nodes.len());

                    let output_e_node = &self.e_nodes[port_address.target_idx];
                    assert!(port_address.port_idx < output_e_node.outputs.len());

                    if output_e_node.wants_execute {
                        assert_eq!(e_input.state, InputState::Changed);
                    }
                };
            }
        }

        assert!(self.e_node_invoke_order.len() <= self.e_node_process_order.len());

        let mut pending_after: HashSet<usize> = self.e_node_invoke_order.iter().copied().collect();
        assert_eq!(pending_after.len(), self.e_node_invoke_order.len());

        for &e_node_idx in self.e_node_invoke_order.iter() {
            assert!(e_node_idx < self.e_nodes.len());
            pending_after.remove(&e_node_idx);

            let e_node = &self.e_nodes[e_node_idx];
            assert_eq!(e_node.process_state, ProcessState::Backward);
            assert!(e_node.wants_execute);
            assert!(!e_node.missing_required_inputs);

            for e_input in e_node.inputs.iter() {
                if let ExecutionBinding::Bind(port_address) = &e_input.binding {
                    assert!(!pending_after.contains(&port_address.target_idx));
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::data::{DynamicValue, StaticValue};
    use crate::function::{test_func_lib, TestFuncHooks};
    use crate::graph::{test_graph, NodeBehavior};
    use common::FileFormat;
    use tokio::sync::Mutex;

    #[test]
    fn basic_run() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        assert_eq!(execution_graph.e_nodes.len(), 5);
        assert_eq!(execution_graph.e_node_process_order.len(), 5);
        assert_eq!(execution_graph.e_node_invoke_order.len(), 5);
        assert!(execution_graph
            .e_nodes
            .iter()
            .all(|e_node| !e_node.missing_required_inputs));
        assert!(execution_graph
            .e_nodes
            .iter()
            .all(|e_node| e_node.wants_execute));

        let get_a = execution_graph.by_name("get_a").unwrap();
        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert_eq!(get_a.outputs[0].usage_count, 1);
        assert_eq!(get_b.outputs[0].usage_count, 2);
        assert_eq!(sum.outputs[0].usage_count, 1);
        assert_eq!(mult.outputs[0].usage_count, 1);

        assert!(!get_a.changed_inputs);
        assert!(!get_b.changed_inputs);
        assert!(sum.changed_inputs);
        assert!(mult.changed_inputs);
        assert!(print.changed_inputs);

        assert!(print.terminal);

        Ok(())
    }

    #[test]
    fn missing_input() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        // this excludes get_a from graph
        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert!(!get_b.missing_required_inputs);
        assert!(sum.missing_required_inputs);
        assert!(mult.missing_required_inputs);
        assert!(print.missing_required_inputs);

        Ok(())
    }

    #[test]
    fn missing_non_required_input() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        // this excludes get_a from graph
        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;
        // this makes mult execute with missing non required input
        func_lib.by_name_mut("mult").unwrap().inputs[0].required = false;

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert!(sum.missing_required_inputs);
        assert!(!mult.missing_required_inputs);
        assert!(!print.missing_required_inputs);

        Ok(())
    }

    #[test]
    fn const_binding() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        // this excludes get_a, get_b and sum from graph
        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(3));
        mult.inputs[1].binding = Binding::Const(StaticValue::Int(5));

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(mult.changed_inputs);
        assert_eq!(mult.inputs[0].state, InputState::Changed);
        assert_eq!(mult.inputs[1].state, InputState::Changed);

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        let mult = execution_graph.by_name("mult").unwrap();
        assert!(!execution_graph.by_name("mult").unwrap().changed_inputs);
        assert_eq!(mult.inputs[0].state, InputState::Unchanged);
        assert_eq!(mult.inputs[1].state, InputState::Unchanged);

        graph.by_name_mut("mult").unwrap().inputs[0].binding = Binding::Const(StaticValue::Int(4));
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert_eq!(execution_graph.e_node_invoke_order.len(), 2);

        assert_eq!(mult.inputs[0].state, InputState::Changed);
        assert_eq!(mult.inputs[1].state, InputState::Unchanged);
        assert!(!mult.missing_required_inputs);
        assert!(!print.missing_required_inputs);
        assert!(mult.changed_inputs);
        assert!(print.changed_inputs);

        Ok(())
    }

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        for format in [FileFormat::Yaml, FileFormat::Json, FileFormat::Lua] {
            let serialized = execution_graph.serialize(format);
            let deserialized = ExecutionGraph::deserialize(serialized.as_str(), format)?;
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

        execution_graph.update(&graph, &func_lib)?;

        let binding1: Binding = (graph.by_name("get_a").unwrap().id, 0).into();
        let binding2: Binding = (graph.by_name("get_b").unwrap().id, 0).into();
        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = binding1;
        mult.inputs[1].binding = binding2;

        execution_graph.update(&graph, &func_lib)?;

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
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        assert!(
            execution_graph
                .e_node_invoke_order
                .iter()
                .any(|e_node_idx| execution_graph.e_nodes[*e_node_idx].name == "get_b"),
            "pure node invoked if no cache values"
        );

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        assert!(
            execution_graph
                .e_node_invoke_order
                .iter()
                .all(|e_node_idx| execution_graph.e_nodes[*e_node_idx].name != "get_b"),
            "pure node not invoked if has cached values"
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn node_skips_consequent_invokations() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || 1),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_result| {}),
        });
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(execution_graph.e_node_invoke_order.len(), 5);

        execution_graph.execute().await?;
        assert_eq!(execution_graph.e_node_invoke_order.len(), 1);

        Ok(())
    }

    #[test]
    fn inpure_node_always_invoked() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());

        graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::AsFunction;
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        assert!(
            execution_graph
                .e_node_invoke_order
                .iter()
                .any(|e_node_idx| execution_graph.e_nodes[*e_node_idx].name == "get_b"),
            "get_b not in e_node_execution_order"
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

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        assert!(
            execution_graph
                .e_node_invoke_order
                .iter()
                .any(|e_node_idx| execution_graph.e_nodes[*e_node_idx].name == "get_b"),
            "once node invoked if has no cached outputs"
        );

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.pre_execute();

        assert!(
            execution_graph
                .e_node_invoke_order
                .iter()
                .all(|e_node_idx| execution_graph.e_nodes[*e_node_idx].name != "get_b"),
            "once node not invoked is has cached outputs"
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
        let err = execution_graph
            .update(&graph, &func_lib)
            .expect_err("Expected cycle detection error");
        match err {
            ExecutionError::CycleDetected { node_id } => {
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
        execution_graph.update(&graph, &func_lib)?;

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
            get_a: Arc::new(move || test_values_a.try_lock().unwrap().a),
            get_b: Arc::new(move || test_values_b.try_lock().unwrap().b),
            print: Arc::new(move |result| {
                test_values_result.try_lock().unwrap().result = result;
            }),
        });

        let graph = test_graph();

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;
        assert_eq!(test_values.try_lock()?.result, 35);

        // get_b is pure, so changing this should not affect result
        test_values.try_lock()?.b = 7;

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;
        assert_eq!(test_values.try_lock()?.result, 35);

        // now result will be different
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(test_values.try_lock()?.result, 63);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn const_binding_invokes_only_once() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || unreachable!()),
            get_b: Arc::new(move || unreachable!()),
            print: Arc::new(move |_result| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        // this excludes get_a and get_b from graph
        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(3));
        mult.inputs[1].binding = Binding::Const(StaticValue::Int(5));

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(
            execution_graph.e_node_invoke_order.iter().len(),
            2,
            "mult should be invoked as no cached values"
        );

        graph.by_name_mut("mult").unwrap().inputs[0].binding = Binding::Const(StaticValue::Int(3));
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(
            execution_graph.e_node_invoke_order.iter().len(),
            1,
            "changing value to the same should not recompute cache"
        );

        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(4));
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(
            execution_graph.e_node_invoke_order.iter().len(),
            2,
            "mult should be invoked as const value changed"
        );

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(
            execution_graph.e_node_invoke_order.iter().len(),
            1,
            "mult should not be invoked again"
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn optional_input_binding_change_recomputes() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || 1),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_result| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        // second input for mult is optional
        // this excludes get_a, get_b and sum
        let sum = graph.by_name_mut("mult").unwrap();
        sum.inputs[0].binding = Binding::Const(2.into());
        sum.inputs[1].binding = Binding::None;

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(
            execution_graph.e_node_invoke_order.iter().len(),
            2,
            "changing binding should recompute sum"
        );

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(
            execution_graph.e_node_invoke_order.iter().len(),
            1,
            "nothing changed so nothing should be recomputed"
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn const_input_excludes_upstream_node() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || 1),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_result| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::Const(33.into());

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(
            execution_graph.e_node_invoke_order.iter().len(),
            4,
            "get_a is excluded"
        );

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[1].binding = Binding::None;

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(
            execution_graph.e_node_invoke_order.iter().len(),
            3,
            "sum binging change so sum should be recomputed"
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn change_from_const_to_bind_recomputes() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || 1),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_result| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        let get_b_id = graph.by_name_mut("get_b").unwrap().id;
        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::Const(33.into());

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = (get_b_id, 0).into();

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(
            execution_graph.e_node_invoke_order.iter().len(),
            3,
            "changed from const to bind should recompute"
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn required_none_binding_execute_is_stable() -> anyhow::Result<()> {
        let func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || 1),
            get_b: Arc::new(move || 11),
            print: Arc::new(move |_result| {}),
        });

        let mut graph = test_graph();
        let mut execution_graph = ExecutionGraph::default();

        let sum = graph.by_name_mut("sum").unwrap();
        sum.inputs[0].binding = Binding::None;

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;
        execution_graph.execute().await?;

        Ok(())
    }
}
