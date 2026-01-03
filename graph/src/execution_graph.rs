use std::mem::take;
use std::ops::{Index, IndexMut};
use std::panic;

use anyhow::Result;
use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use serde_yml::modules::error::new;
use thiserror::Error;

use crate::context::ContextManager;
use crate::data::{DataType, DynamicValue, StaticValue};
use crate::function::{Func, FuncBehavior, FuncLib, InvokeCache, InvokeInput};
use crate::graph::{Binding, Graph, Node, NodeBehavior, NodeId, PortAddress};
use crate::prelude::{FuncId, FuncLambda};
use common::{is_debug, FileFormat};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum InputState {
    #[default]
    None,
    Changed,
    Unchanged,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputUsage {
    #[default]
    Skip,
    Needed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionBinding {
    #[default]
    None,
    Const(StaticValue),
    Bind(ExecutionPortAddress),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum ProcessState {
    #[default]
    None,
    Processing,
    Backward1,
    Forward,
    Backward2,
}
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: NodeId,

    inited: bool,
    process_state: ProcessState,

    pub terminal: bool,
    pub missing_required_inputs: bool,
    pub changed_inputs: bool,
    pub behavior: ExecutionBehavior,

    pub inputs: Vec<ExecutionInput>,
    pub outputs: Vec<ExecutionOutput>,

    pub func_id: FuncId,
    pub wants_execute: bool,

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
    pub e_node_invoke_order: Vec<usize>,

    #[serde(skip)]
    ctx_manager: ContextManager,

    //caches
    #[serde(skip)]
    stack: Vec<Visit>,
}

impl ExecutionNode {
    fn reset(&mut self) {
        self.terminal = false;
        self.missing_required_inputs = false;
        self.changed_inputs = false;
        self.process_state = ProcessState::None;
        self.wants_execute = false;
        self.run_time = 0.0;
        self.error = None;
        self.outputs.fill(ExecutionOutput::default());
    }
    fn update(&mut self, node: &Node, func: &Func) {
        assert_eq!(self.id, node.id);
        assert_eq!(node.inputs.len(), func.inputs.len());

        self.func_id = func.id;
        self.lambda = func.lambda.clone();
        self.terminal = node.terminal;

        self.behavior = match node.behavior {
            NodeBehavior::AsFunction => match func.behavior {
                FuncBehavior::Pure => ExecutionBehavior::Pure,
                FuncBehavior::Impure => ExecutionBehavior::Impure,
            },
            NodeBehavior::Once => ExecutionBehavior::Once,
        };

        if !self.inited {
            self.inited = true;

            self.inputs.reserve(func.inputs.len());
            if self.inputs.len() != node.inputs.len() {
                self.inputs.clear();
                for func_input in func.inputs.iter() {
                    self.inputs.push(ExecutionInput {
                        state: InputState::None,
                        required: func_input.required,
                        binding: ExecutionBinding::None,
                        data_type: func_input.data_type.clone(),
                    });
                }
            }

            self.outputs.clear();
            self.outputs
                .resize(func.outputs.len(), ExecutionOutput::default());

            self.output_values = None;
        }

        assert_eq!(self.inputs.len(), node.inputs.len());
        assert_eq!(self.inputs.len(), func.inputs.len());
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

            let e_node = &mut self.e_nodes[e_node_idx];
            e_node.inited = false;
            e_node.output_values = None;

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

    // Rebuild execution state
    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) -> ExecutionResult<()> {
        validate_execution_inputs(graph, func_lib);

        self.e_nodes.iter_mut().for_each(|e_node| e_node.reset());

        self.backward1(graph, func_lib)?;
        self.forward();
        self.backward2();

        self.validate_with(graph, func_lib);

        Ok(())
    }

    pub async fn execute(&mut self) -> ExecutionResult<ExecutionStats> {
        // self.backward2();

        let start = std::time::Instant::now();

        let mut inputs: Vec<InvokeInput> = Vec::default();
        let mut output_usage: Vec<OutputUsage> = Vec::default();
        let mut error: Option<ExecutionError> = None;

        for e_node_idx in self.e_node_invoke_order.iter().copied() {
            let e_node = &self.e_nodes[e_node_idx];
            assert!(e_node.inited);

            inputs.clear();
            for input in e_node.inputs.iter() {
                let value: DynamicValue = match &input.binding {
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
                if output.usage_count == 0 {
                    OutputUsage::Skip
                } else {
                    OutputUsage::Needed
                }
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

    // Walk upstream dependencies to collect active nodes in processing order for input-state evaluation.
    fn backward1(&mut self, graph: &Graph, func_lib: &FuncLib) -> ExecutionResult<()> {
        self.e_node_invoke_order.reserve(graph.nodes.len());
        self.e_node_invoke_order.clear();
        self.stack.reserve(graph.nodes.len());

        let mut write_idx = 0;
        let mut stack: Vec<Visit> = take(&mut self.stack);
        stack.clear();

        for node in graph.nodes.iter().filter(|&node| node.terminal) {
            let e_node_idx = self
                .e_nodes
                .compact_insert_with(&node.id, &mut write_idx, || ExecutionNode {
                    id: node.id,
                    ..Default::default()
                });

            stack.push(Visit {
                e_node_idx,
                cause: VisitCause::Terminal,
            });
        }

        while let Some(visit) = stack.pop() {
            let output_request = match visit.cause {
                VisitCause::Terminal => None,
                VisitCause::OutputRequest { output_idx } => Some(output_idx),
                VisitCause::Done => {
                    let e_node = &mut self.e_nodes[visit.e_node_idx];
                    assert_eq!(e_node.process_state, ProcessState::Processing);
                    e_node.process_state = ProcessState::Backward1;
                    self.e_node_invoke_order.push(visit.e_node_idx);
                    continue;
                }
            };

            let already_processed = {
                let e_node = &self.e_nodes[visit.e_node_idx];
                match e_node.process_state {
                    ProcessState::None => false,
                    ProcessState::Processing => {
                        return Err(ExecutionError::CycleDetected { node_id: e_node.id });
                    }
                    ProcessState::Backward1 => true,
                    ProcessState::Forward | ProcessState::Backward2 => unreachable!(),
                }
            };

            if !already_processed {
                let node = {
                    let e_node = &mut self.e_nodes[visit.e_node_idx];
                    let node = graph.by_id(&e_node.id).unwrap();
                    let func = func_lib.by_id(&node.func_id).unwrap();
                    e_node.update(node, func);
                    e_node.process_state = ProcessState::Processing;
                    stack.push(Visit {
                        e_node_idx: visit.e_node_idx,
                        cause: VisitCause::Done,
                    });

                    node
                };

                for (input_idx, input) in node.inputs.iter().enumerate() {
                    {
                        let e_input = &mut self.e_nodes[visit.e_node_idx].inputs[input_idx];

                        match (&input.binding, &e_input.binding) {
                            (Binding::None, ExecutionBinding::None) => {
                                e_input.state = InputState::None;
                            }
                            (Binding::Const(value), ExecutionBinding::Const(existing))
                                if value == existing =>
                            {
                                e_input.state = InputState::Unchanged;
                            }
                            (Binding::Const(value), _) => {
                                e_input.state = InputState::Changed;
                                e_input.binding = ExecutionBinding::Const(value.clone());
                            }
                            (_, _) => {
                                e_input.state = InputState::None;
                            }
                        };
                    }

                    {
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

                        let desired_binding = ExecutionBinding::Bind(ExecutionPortAddress {
                            target_idx: output_e_node_idx,
                            port_idx: port_address.port_idx,
                        });
                        let e_input = &mut self.e_nodes[visit.e_node_idx].inputs[input_idx];
                        if e_input.binding != desired_binding {
                            e_input.binding = desired_binding;
                        }

                        stack.push(Visit {
                            e_node_idx: output_e_node_idx,
                            cause: VisitCause::OutputRequest {
                                output_idx: port_address.port_idx,
                            },
                        });
                    }
                }
            }

            if let Some(output_idx) = output_request {
                let e_node = &mut self.e_nodes[visit.e_node_idx];
                assert!(e_node.inited);
                e_node.outputs[output_idx].usage_count += 1;
            }
        }

        self.stack = take(&mut stack);
        self.e_nodes.compact_finish(write_idx);

        Ok(())
    }

    // Propagate input state forward through the processing order.
    fn forward(&mut self) {
        for e_node_idx in self.e_node_invoke_order.iter().copied() {
            let mut changed_inputs = false;
            let mut missing_required_inputs = false;

            for input_idx in 0..self.e_nodes[e_node_idx].inputs.len() {
                let e_input = &self.e_nodes[e_node_idx].inputs[input_idx];
                let input_state = match &e_input.binding {
                    ExecutionBinding::Bind(port_address) => {
                        let output_e_node = &self.e_nodes[port_address.target_idx];

                        assert_eq!(output_e_node.process_state, ProcessState::Forward);
                        assert!(output_e_node.inited);
                        assert!(port_address.port_idx < output_e_node.outputs.len());

                        if output_e_node.missing_required_inputs {
                            assert!(!output_e_node.wants_execute);
                            InputState::None
                        } else if output_e_node.wants_execute {
                            InputState::Changed
                        } else {
                            InputState::Unchanged
                        }
                    }
                    _ => e_input.state,
                };

                let e_input = &mut self.e_nodes[e_node_idx].inputs[input_idx];
                e_input.state = input_state;

                match input_state {
                    InputState::Unchanged => {}
                    InputState::Changed => changed_inputs = true,
                    InputState::None => missing_required_inputs |= e_input.required,
                }
            }

            let e_node = &mut self.e_nodes[e_node_idx];
            assert_eq!(e_node.process_state, ProcessState::Backward1);
            assert!(e_node.inited);

            e_node.process_state = ProcessState::Forward;
            e_node.changed_inputs = changed_inputs;
            e_node.missing_required_inputs = missing_required_inputs;
            e_node.wants_execute = !missing_required_inputs
                && match e_node.behavior {
                    ExecutionBehavior::Impure => true,
                    ExecutionBehavior::Pure => e_node.output_values.is_none() || changed_inputs,
                    ExecutionBehavior::Once => e_node.output_values.is_none(),
                };
        }
    }

    // Walk upstream dependencies to collect the execution order.
    fn backward2(&mut self) {
        self.e_node_invoke_order.clear();

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
                    e_node.process_state = ProcessState::Backward2;
                    continue;
                }
            };

            match e_node.process_state {
                ProcessState::Processing | ProcessState::None | ProcessState::Backward1 => {
                    unreachable!()
                }
                ProcessState::Forward => {}
                ProcessState::Backward2 => continue,
            }

            if !e_node.wants_execute {
                e_node.process_state = ProcessState::Backward2;
                continue;
            }

            e_node.process_state = ProcessState::Processing;
            stack.push(Visit {
                e_node_idx: visit.e_node_idx,
                cause: VisitCause::Done,
            });

            let e_node = &self.e_nodes[visit.e_node_idx];
            for input in e_node.inputs.iter() {
                match input.state {
                    InputState::None | InputState::Unchanged => continue,
                    InputState::Changed => {}
                }

                let Some(port_address) = input.binding.as_bind() else {
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

    pub fn validate_with(&self, graph: &Graph, func_lib: &FuncLib) {
        if !is_debug() {
            return;
        }

        assert!(self.e_nodes.len() <= graph.nodes.len());

        let mut seen_node_ids: HashSet<NodeId> = HashSet::with_capacity(self.e_nodes.len());
        for (e_node_idx, e_node) in self.e_nodes.iter().enumerate() {
            assert!(e_node.inited);
            assert!(!seen_node_ids.contains(&e_node.id));
            seen_node_ids.insert(e_node.id);

            if let Some(output_values) = e_node.output_values.as_ref() {
                assert_eq!(output_values.len(), e_node.outputs.len());
            }

            if self.e_node_invoke_order.contains(&e_node_idx) {
                assert_eq!(e_node.process_state, ProcessState::Backward2);
            } else {
                assert!(
                    e_node.process_state == ProcessState::Forward
                        || e_node.process_state == ProcessState::Backward2
                );
            }

            let node = graph.by_id(&e_node.id).unwrap();
            let func = func_lib.by_id(&e_node.func_id).unwrap();

            assert_eq!(e_node.func_id, node.func_id);
            assert_eq!(node.id, e_node.id);
            assert_eq!(node.func_id, func.id);
            assert_eq!(e_node.inputs.len(), node.inputs.len());
            assert_eq!(e_node.outputs.len(), func.outputs.len());

            // it cannot be missing_required_inputs and wants_execute
            assert!(!(e_node.wants_execute && e_node.missing_required_inputs));

            let missing_required_inputs = e_node
                .inputs
                .iter()
                .any(|input| input.required && input.state == InputState::None);
            assert_eq!(missing_required_inputs, e_node.missing_required_inputs);
            let changed_inputs = e_node
                .inputs
                .iter()
                .any(|input| input.state == InputState::Changed);
            assert_eq!(changed_inputs, e_node.changed_inputs);

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
                        assert_eq!(port_address.port_idx, execution_port_address.port_idx);
                        assert_eq!(
                            port_address.target_id,
                            self.e_nodes[execution_port_address.target_idx].id
                        );
                    }
                    (_, _) => panic!("Mismatched bindings"),
                }
            }
        }

        for idx in 0..self.e_node_invoke_order.len() {
            let e_node_idx = self.e_node_invoke_order[idx];
            let e_node = &self.e_nodes[e_node_idx];
            assert!(!e_node.missing_required_inputs);

            let all_dependencies_in_order = e_node
                .inputs
                .iter()
                .filter_map(|input| match &input.binding {
                    ExecutionBinding::Bind(port_address) => Some(port_address),
                    ExecutionBinding::None | ExecutionBinding::Const(_) => None,
                })
                .all(|port_address| {
                    !self.e_node_invoke_order[idx..].contains(&port_address.target_idx)
                });
            assert!(all_dependencies_in_order);
        }
    }
}

fn validate_execution_inputs(graph: &Graph, func_lib: &FuncLib) {
    if !is_debug() {
        return;
    }

    graph.validate();

    for node in graph.nodes.iter() {
        let func = func_lib.by_id(&node.func_id).unwrap();
        assert_eq!(node.inputs.len(), func.inputs.len());

        for input in node.inputs.iter() {
            if let Binding::Bind(port_address) = &input.binding {
                let output_node = graph.by_id(&port_address.target_id).unwrap();
                let output_func = func_lib.by_id(&output_node.func_id).unwrap();
                assert!(port_address.port_idx < output_func.outputs.len());
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;

    use super::*;
    use crate::data::{DynamicValue, StaticValue};
    use crate::function::{test_func_lib, TestFuncHooks};
    use crate::graph::{test_graph, Input, NodeBehavior};
    use common::FileFormat;
    use tokio::sync::Mutex;

    #[test]
    fn simple_run() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        assert_eq!(execution_graph.e_nodes.len(), 5);
        assert_eq!(execution_graph.e_node_invoke_order.len(), 5);
        assert!(execution_graph
            .e_nodes
            .iter()
            .all(|e_node| !e_node.missing_required_inputs));

        let get_a = execution_graph.by_name("get_a").unwrap();
        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert_eq!(get_a.outputs.len(), 1);
        assert_eq!(get_b.outputs.len(), 1);
        assert_eq!(sum.outputs.len(), 1);
        assert_eq!(mult.outputs.len(), 1);
        assert!(print.outputs.is_empty());

        assert_eq!(get_a.outputs[0].usage_count, 1);
        assert_eq!(get_b.outputs[0].usage_count, 2);
        assert_eq!(sum.outputs[0].usage_count, 1);
        assert_eq!(mult.outputs[0].usage_count, 1);

        assert!(mult.output_values.is_none());

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

        assert_eq!(execution_graph.e_nodes.len(), 4);
        assert!(execution_graph.by_name("get_a").is_none());

        let get_b = execution_graph.by_name("get_b").unwrap();
        let sum = execution_graph.by_name("sum").unwrap();
        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

        assert!(!get_b.missing_required_inputs);
        assert!(sum.missing_required_inputs);
        assert!(mult.missing_required_inputs);
        assert!(print.missing_required_inputs);

        assert!(!get_b.changed_inputs);
        assert!(sum.changed_inputs);

        assert_eq!(sum.inputs[0].state, InputState::None);
        assert_eq!(mult.inputs[0].state, InputState::None);
        assert_eq!(print.inputs[0].state, InputState::None);

        assert_eq!(get_b.outputs.len(), 1);
        assert_eq!(sum.outputs.len(), 1);
        assert_eq!(mult.outputs.len(), 1);
        assert!(print.outputs.is_empty());

        assert_eq!(get_b.outputs[0].usage_count, 2);
        assert_eq!(sum.outputs[0].usage_count, 1);
        assert_eq!(mult.outputs[0].usage_count, 1);

        Ok(())
    }

    #[test]
    fn const_binding() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());
        let mut execution_graph = ExecutionGraph::default();

        // this excludes get_a from graph
        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(3));
        mult.inputs[1].binding = Binding::Const(StaticValue::Int(5));

        {
            execution_graph.update(&graph, &func_lib)?;
            let mult = execution_graph.by_name("mult").unwrap();
            assert!(mult.changed_inputs);
        }

        {
            execution_graph.update(&graph, &func_lib)?;
            let mult = execution_graph.by_name("mult").unwrap();
            assert!(!mult.changed_inputs);
        }

        let mult = graph.by_name_mut("mult").unwrap();
        mult.inputs[0].binding = Binding::Const(StaticValue::Int(4));
        execution_graph.update(&graph, &func_lib)?;

        assert_eq!(execution_graph.e_nodes.len(), 2);
        assert!(execution_graph.by_name("get_a").is_none());
        assert!(execution_graph.by_name("get_b").is_none());
        assert!(execution_graph.by_name("sum").is_none());

        let mult = execution_graph.by_name("mult").unwrap();
        let print = execution_graph.by_name("print").unwrap();

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

        //avoid serialization of e_node_idx_by_id as deserialization order is not guaranteed
        execution_graph.e_nodes.idx_by_key.clear();

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

        graph.by_name_mut("mult").unwrap().inputs = vec![
            Input {
                binding: (graph.by_name("get_a").unwrap().id, 0).into(),
                default_value: None,
            },
            Input {
                binding: (graph.by_name("get_b").unwrap().id, 0).into(),
                default_value: None,
            },
        ];

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        assert_eq!(execution_graph.e_nodes.len(), 4);

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

        // pure node invoked if no cache values
        assert!(execution_graph
            .e_node_invoke_order
            .iter()
            .any(|e_node_idx| execution_graph.e_nodes[*e_node_idx].name == "get_b"));

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib)?;

        // pure node not invoked if has cached values
        assert!(execution_graph
            .e_node_invoke_order
            .iter()
            .all(|e_node_idx| execution_graph.e_nodes[*e_node_idx].name != "get_b"));

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

        // get_b not in e_node_execution_order
        assert!(execution_graph
            .e_node_invoke_order
            .iter()
            .any(|e_node_idx| execution_graph.e_nodes[*e_node_idx].name == "get_b"));

        Ok(())
    }

    #[test]
    fn once_node_always_caches() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let mut func_lib = test_func_lib(TestFuncHooks::default());

        graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::Once;
        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        // once node invoked is has no cached outputs
        assert!(execution_graph
            .e_node_invoke_order
            .iter()
            .any(|e_node_idx| execution_graph.e_nodes[*e_node_idx].name == "get_b"));

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);
        execution_graph.update(&graph, &func_lib)?;

        assert!(execution_graph
            .by_name_mut("mult")
            .unwrap()
            .output_values
            .is_none());

        // once node not invoked is has cached outputs
        assert!(execution_graph
            .e_node_invoke_order
            .iter()
            .all(|e_node_idx| execution_graph.e_nodes[*e_node_idx].name != "get_b"));

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

        let get_a = graph.by_name("get_a").unwrap().id;
        let get_b = graph.by_name("get_b").unwrap().id;
        let sum = graph.by_name("sum").unwrap().id;
        let mult = graph.by_name("mult").unwrap().id;
        let print = graph.by_name("print").unwrap().id;

        execution_graph.invalidate_recursively(vec![sum]);

        assert!(execution_graph.by_id(&get_a).unwrap().inited);
        assert!(execution_graph.by_id(&get_b).unwrap().inited);
        assert!(!execution_graph.by_id(&sum).unwrap().inited);
        assert!(!execution_graph.by_id(&mult).unwrap().inited);
        assert!(!execution_graph.by_id(&print).unwrap().inited);

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

        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        assert_eq!(test_values.try_lock()?.result, 63);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cached_value() -> anyhow::Result<()> {
        let test_values = Arc::new(Mutex::new(TestValues {
            a: 2,
            b: 5,
            result: 0,
        }));

        let test_values_a = test_values.clone();
        let test_values_b = test_values.clone();
        let test_values_result = test_values.clone();
        let mut func_lib = test_func_lib(TestFuncHooks {
            get_a: Arc::new(move || {
                let mut guard = test_values_a.try_lock().unwrap();
                let a1 = guard.a;
                guard.a += 1;

                a1
            }),
            get_b: Arc::new(move || {
                let mut guard = test_values_b.try_lock().unwrap();
                let b1 = guard.b;
                guard.b += 1;
                if b1 == 6 {
                    panic!("Unexpected call to get_b");
                }

                b1
            }),
            print: Arc::new(move |result| {
                test_values_result.try_lock().unwrap().result = result;
            }),
        });

        let mut graph = test_graph();
        func_lib.by_name_mut("get_a").unwrap().behavior = FuncBehavior::Impure;
        graph.by_name_mut("get_a").unwrap().behavior = NodeBehavior::AsFunction;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        // assert that both nodes were called
        {
            let guard = test_values.try_lock()?;
            assert_eq!(guard.a, 3);
            assert_eq!(guard.b, 6);
            assert_eq!(guard.result, 35);
        }

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute().await?;

        // assert that node was called again
        let guard = test_values.try_lock()?;
        assert_eq!(guard.a, 4);
        // but node b was cached
        assert_eq!(guard.b, 6);
        assert_eq!(guard.result, 40);

        Ok(())
    }
}
