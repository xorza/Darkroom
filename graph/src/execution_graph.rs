use std::mem::take;
use std::ops::{Index, IndexMut};
use std::panic;

use anyhow::Result;
use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::args::Args;
use crate::data::{DataType, DynamicValue};
use crate::function::{Func, FuncBehavior, FuncLib, InvokeCache};
use crate::graph::{Binding, Graph, Node, NodeBehavior, NodeId};
use crate::prelude::FuncId;
use common::{is_debug, FileFormat};

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum ExecutionError {
    #[error("Function invocation failed for function {function_id:?}: {message}")]
    Invoke {
        function_id: FuncId,
        message: String,
    },
    #[error("Cycle detected while building execution graph at node {node_id:?}")]
    CycleDetected { node_id: NodeId },
}

pub type ExecutionResult<T> = std::result::Result<T, ExecutionError>;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PortAddress {
    pub e_node_idx: usize,
    pub port_idx: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum InputState {
    #[default]
    Unknown,
    Unchanged,
    Changed,
    Missing,
}
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExecutionInput {
    pub state: InputState,
    pub required: bool,
    pub output_address: Option<PortAddress>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExecutionOutput {
    #[default]
    Unused,
    Used,
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
    Done { execute: bool },
}
#[derive(Debug)]
struct Visit {
    node_idx: usize,
    e_node_idx: usize,
    cause: VisitCause,
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

    pub has_missing_inputs: bool,
    pub has_changed_inputs: bool,
    pub behavior: ExecutionBehavior,

    pub node_idx: usize,
    pub func_idx: usize,
    pub inputs: Vec<ExecutionInput>,
    pub outputs: Vec<ExecutionOutput>,

    process_state: ProcessState,

    pub run_time: f64,
    pub error: Option<ExecutionError>,

    #[serde(skip)]
    pub(crate) cache: InvokeCache,
    #[serde(skip)]
    pub(crate) output_values: Option<Vec<DynamicValue>>,

    #[cfg(debug_assertions)]
    pub name: String,
}
impl KeyIndexKey<NodeId> for ExecutionNode {
    fn key(&self) -> NodeId {
        self.id
    }
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub e_nodes: KeyIndexVec<NodeId, ExecutionNode>,
    e_node_processing_order: Vec<usize>,
    pub e_node_execution_order: Vec<usize>,

    //caches
    #[serde(skip)]
    stack: Vec<Visit>,
    #[serde(skip)]
    node_idx_by_id: HashMap<NodeId, usize>,
}

impl ExecutionNode {
    fn reset(&mut self) {
        let mut prev_state = take(self);

        self.cache = take(&mut prev_state.cache);
        self.output_values = take(&mut prev_state.output_values);
        self.inputs = take(&mut prev_state.inputs);
        self.outputs = take(&mut prev_state.outputs);

        #[cfg(debug_assertions)]
        {
            self.name = take(&mut prev_state.name);
        }
    }
    fn refresh(&mut self, node: &Node, func: &Func, node_idx: usize, func_idx: usize) {
        self.id = node.id;
        self.node_idx = node_idx;
        self.func_idx = func_idx;

        self.behavior = match node.behavior {
            NodeBehavior::AsFunction => match func.behavior {
                FuncBehavior::Pure => ExecutionBehavior::Pure,
                FuncBehavior::Impure => ExecutionBehavior::Impure,
            },
            NodeBehavior::Once => ExecutionBehavior::Once,
        };

        self.inputs.clear();
        self.inputs.reserve(func.inputs.len());
        self.inputs
            .extend(func.inputs.iter().map(|input| ExecutionInput {
                required: input.required,
                ..Default::default()
            }));

        self.outputs.clear();
        self.outputs
            .resize(func.outputs.len(), ExecutionOutput::default());

        #[cfg(debug_assertions)]
        {
            self.name.clear();
            self.name.push_str(&node.name);
        }
    }
}

impl ExecutionGraph {
    pub fn by_id(&self, node_id: NodeId) -> Option<&ExecutionNode> {
        self.e_nodes.iter().find(|node| node.id == node_id)
    }
    pub fn by_id_mut(&mut self, node_id: NodeId) -> Option<&mut ExecutionNode> {
        self.e_nodes.iter_mut().find(|node| node.id == node_id)
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

    // Rebuild execution state from the current graph and function library.
    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) -> ExecutionResult<()> {
        validate_execution_inputs(graph, func_lib);

        self.node_idx_by_id.clear();
        self.node_idx_by_id.extend(
            graph
                .nodes
                .iter()
                .enumerate()
                .map(|(idx, node)| (node.id, idx)),
        );

        self.stack.clear();
        self.e_node_processing_order.clear();
        self.e_node_execution_order.clear();
        self.e_nodes.iter_mut().for_each(|e_node| e_node.reset());

        self.backward1(graph, func_lib)?;
        self.forward(graph);
        self.backward2(graph);

        self.node_idx_by_id.clear();

        self.validate_with_graph(graph, func_lib);

        Ok(())
    }

    pub fn execute(&mut self, graph: &Graph, func_lib: &FuncLib) -> ExecutionResult<()> {
        let mut inputs: Args = Args::default();

        for e_node_idx in self.e_node_execution_order.iter().copied() {
            let (node, func) = {
                let e_node = &self.e_nodes[e_node_idx];
                let node = &graph.nodes[e_node.node_idx];
                let func = &func_lib.funcs[e_node.func_idx];

                (node, func)
            };

            inputs.resize_and_clear(node.inputs.len());
            for (input_idx, input) in node.inputs.iter().enumerate() {
                let value: DynamicValue = match &input.binding {
                    Binding::None => DynamicValue::None,
                    Binding::Const => input
                        .const_value
                        .as_ref()
                        .expect("Const value is not set")
                        .into(),

                    Binding::Output(output_binding) => {
                        let output_address = self.e_nodes[e_node_idx].inputs[input_idx]
                            .output_address
                            .clone()
                            .expect("Output address is not set");
                        assert_eq!(output_binding.output_idx, output_address.port_idx);

                        let output_values = self.e_nodes[output_address.e_node_idx]
                            .output_values
                            .as_mut()
                            .expect("Output values missing for bound node; check execution order");

                        output_values[output_binding.output_idx].clone()
                    }
                };

                let data_type = &func.inputs[input_idx].data_type;
                inputs[input_idx] = convert_type(&value, data_type);
            }

            let e_node = &mut self.e_nodes[e_node_idx];
            let outputs = e_node
                .output_values
                .get_or_insert_with(|| vec![DynamicValue::None; func.outputs.len()]);

            let start = std::time::Instant::now();
            let invoke_result = func_lib
                .invoke_by_index(
                    e_node.func_idx,
                    &mut e_node.cache,
                    inputs.as_slice(),
                    outputs.as_mut_slice(),
                )
                .map_err(|source| ExecutionError::Invoke {
                    function_id: node.func_id,
                    message: source.to_string(),
                });
            e_node.run_time = start.elapsed().as_secs_f64();
            if let Err(error) = invoke_result {
                e_node.error = Some(error.clone());
                return Err(error);
            }
            e_node.error = None;

            inputs.clear();
        }

        Ok(())
    }

    // Walk upstream dependencies to collect active nodes in processing order for input-state evaluation.
    fn backward1(&mut self, graph: &Graph, func_lib: &FuncLib) -> ExecutionResult<()> {
        self.e_node_processing_order.reserve(graph.nodes.len());

        let mut write_idx = 0;
        let mut stack: Vec<Visit> = take(&mut self.stack);
        stack.reserve(10);

        for (node_idx, node) in graph
            .nodes
            .iter()
            .enumerate()
            .filter(|&(_, node)| node.terminal)
        {
            let e_node_idx = self.e_nodes.compact_insert_default(node.id, &mut write_idx);

            stack.push(Visit {
                node_idx,
                e_node_idx,
                cause: VisitCause::Terminal,
            });
        }

        while let Some(visit) = stack.pop() {
            let e_node_idx = visit.e_node_idx;
            let node = &graph.nodes[visit.node_idx];

            let e_node = &mut self.e_nodes[e_node_idx];
            match visit.cause {
                VisitCause::Terminal => {}
                VisitCause::OutputRequest { output_idx } => {
                    if e_node.process_state != ProcessState::None {
                        e_node.outputs[output_idx] = ExecutionOutput::Used
                    }
                }
                VisitCause::Done { .. } => {
                    assert_eq!(e_node.process_state, ProcessState::Processing);
                    e_node.process_state = ProcessState::Backward1;
                    self.e_node_processing_order.push(e_node_idx);
                    continue;
                }
            };
            match e_node.process_state {
                ProcessState::Backward1 => {
                    continue;
                }
                ProcessState::Processing => {
                    return Err(ExecutionError::CycleDetected { node_id: e_node.id });
                }
                ProcessState::None => {}
                ProcessState::Forward | ProcessState::Backward2 => {
                    panic!("Invalid processing state")
                }
            }

            e_node.process_state = ProcessState::Processing;
            stack.push(Visit {
                node_idx: visit.node_idx,
                e_node_idx,
                cause: VisitCause::Done { execute: false },
            });

            let func_idx = func_lib
                .funcs
                .iter()
                .position(|func| func.id == node.func_id)
                .expect("FuncLib missing function for graph node func_id");
            let func = &func_lib.funcs[func_idx];
            e_node.refresh(node, func, visit.node_idx, func_idx);
            if let VisitCause::OutputRequest { output_idx } = visit.cause {
                e_node.outputs[output_idx] = ExecutionOutput::Used
            }

            for (input_idx, input) in node.inputs.iter().enumerate() {
                if let Binding::Output(output_binding) = &input.binding {
                    let output_e_node_idx = self
                        .e_nodes
                        .compact_insert_default(output_binding.output_node_id, &mut write_idx);
                    self.e_nodes[e_node_idx].inputs[input_idx].output_address = Some(PortAddress {
                        e_node_idx: output_e_node_idx,
                        port_idx: output_binding.output_idx,
                    });
                    let output_node_idx = self.node_idx_by_id[&output_binding.output_node_id];
                    stack.push(Visit {
                        node_idx: output_node_idx,
                        e_node_idx: output_e_node_idx,
                        cause: VisitCause::OutputRequest {
                            output_idx: output_binding.output_idx,
                        },
                    });
                }
            }
        }

        self.stack = take(&mut stack);
        self.e_nodes.compact_finish(write_idx);

        if is_debug() {
            assert_eq!(self.e_nodes.len(), self.e_nodes.idx_by_key.len());
            assert!(self.e_nodes.len() <= graph.nodes.len());
            self.e_nodes.iter().enumerate().for_each(|(idx, e_node)| {
                assert!(e_node.node_idx < graph.nodes.len());
                assert_eq!(graph.nodes[e_node.node_idx].id, e_node.id);
                assert_eq!(idx, self.e_nodes.idx_by_key[&e_node.id]);
            });
        }

        Ok(())
    }

    // Propagate input state forward through the processing order.
    fn forward(&mut self, graph: &Graph) {
        for e_node_idx in self.e_node_processing_order.iter().copied() {
            let node = &graph.nodes[self.e_nodes[e_node_idx].node_idx];

            let mut has_changed_inputs = false;
            let mut has_missing_inputs = false;

            for (input_idx, input) in node.inputs.iter().enumerate() {
                let input_state = match &input.binding {
                    Binding::None => InputState::Missing,
                    // todo Const bindings are treated as changed each run until change tracking exists.
                    Binding::Const => InputState::Changed,
                    Binding::Output(_) => {
                        let output_e_node_idx = self.e_nodes[e_node_idx].inputs[input_idx]
                            .output_address
                            .as_ref()
                            .expect("Output binding references missing execution node")
                            .e_node_idx;
                        let output_e_node = &self.e_nodes[output_e_node_idx];

                        assert_eq!(output_e_node.process_state, ProcessState::Forward);

                        if output_e_node.has_missing_inputs {
                            InputState::Missing
                        } else {
                            let output_cached = output_e_node.output_values.is_some();
                            match output_e_node.behavior {
                                ExecutionBehavior::Impure => InputState::Changed,
                                ExecutionBehavior::Pure => {
                                    if output_e_node.has_changed_inputs || !output_cached {
                                        InputState::Changed
                                    } else {
                                        InputState::Unchanged
                                    }
                                }
                                ExecutionBehavior::Once => {
                                    if output_cached {
                                        InputState::Unchanged
                                    } else {
                                        InputState::Changed
                                    }
                                }
                            }
                        }
                    }
                };

                let e_node_input = &mut self.e_nodes[e_node_idx].inputs[input_idx];
                e_node_input.state = input_state;
                match input_state {
                    InputState::Unchanged => {}
                    InputState::Changed => has_changed_inputs = true,
                    InputState::Missing => has_missing_inputs |= e_node_input.required,
                    InputState::Unknown => panic!("unprocessed input"),
                }
            }

            let e_node = &mut self.e_nodes[e_node_idx];
            assert_eq!(e_node.process_state, ProcessState::Backward1);

            e_node.has_changed_inputs = has_changed_inputs;
            e_node.has_missing_inputs = has_missing_inputs;
            e_node.process_state = ProcessState::Forward;
        }
    }

    // Walk upstream dependencies to collect the execution order.
    fn backward2(&mut self, graph: &Graph) {
        self.e_node_processing_order
            .reserve(self.e_node_processing_order.len());

        let mut stack: Vec<Visit> = take(&mut self.stack);
        assert!(stack.is_empty());

        for (idx, e_node) in self.e_nodes.iter().enumerate() {
            if graph.nodes[e_node.node_idx].terminal {
                stack.push(Visit {
                    node_idx: 0,
                    e_node_idx: idx,
                    cause: VisitCause::Terminal,
                });
            }
        }

        while let Some(visit) = stack.pop() {
            let e_node = &mut self.e_nodes[visit.e_node_idx];

            match visit.cause {
                VisitCause::Terminal | VisitCause::OutputRequest { .. } => {}
                VisitCause::Done { execute } => {
                    e_node.process_state = ProcessState::Backward2;
                    if execute {
                        self.e_node_execution_order.push(visit.e_node_idx);
                    }
                    continue;
                }
            };

            match e_node.process_state {
                ProcessState::None | ProcessState::Backward1 => {
                    panic!("Expected a processed node")
                }
                ProcessState::Processing => panic!(
                    "Cycle detected too late. Should have been caught earlier in backward1()"
                ),
                ProcessState::Forward => {}
                ProcessState::Backward2 => continue,
            }

            let execute = match e_node.behavior {
                ExecutionBehavior::Impure => true,
                ExecutionBehavior::Pure => {
                    e_node.output_values.is_none() || e_node.has_changed_inputs
                }
                ExecutionBehavior::Once => e_node.output_values.is_none(),
            } && !e_node.has_missing_inputs;

            e_node.process_state = ProcessState::Processing;
            stack.push(Visit {
                node_idx: 0,
                e_node_idx: visit.e_node_idx,
                cause: VisitCause::Done { execute },
            });

            if execute {
                for input in e_node.inputs.iter() {
                    if match input.state {
                        InputState::Unchanged | InputState::Missing => false,
                        InputState::Changed => true,
                        InputState::Unknown => panic!("Unprocessed input"),
                    } {
                        if let Some(output_address) = input.output_address.as_ref() {
                            stack.push(Visit {
                                node_idx: 0,
                                e_node_idx: output_address.e_node_idx,
                                cause: VisitCause::OutputRequest { output_idx: 0 },
                            });
                        }
                    }
                }
            }
        }

        self.stack = take(&mut stack);
    }

    pub fn validate_with_graph(&self, graph: &Graph, func_lib: &FuncLib) {
        if !is_debug() {
            return;
        }

        assert!(self.e_nodes.len() <= graph.nodes.len());

        let mut seen_node_indices = vec![false; graph.nodes.len()];
        for (e_node_idx, e_node) in self.e_nodes.iter().enumerate() {
            assert!(e_node.node_idx < graph.nodes.len());
            assert!(!seen_node_indices[e_node.node_idx]);
            seen_node_indices[e_node.node_idx] = true;

            if self.e_node_execution_order.contains(&e_node_idx) {
                assert_eq!(e_node.process_state, ProcessState::Backward2);
            } else {
                assert!(
                    e_node.process_state == ProcessState::Forward
                        || e_node.process_state == ProcessState::Backward2
                );
            }

            assert!(e_node.func_idx < func_lib.funcs.len());
            let node = &graph.nodes[e_node.node_idx];
            let func = &func_lib.funcs[e_node.func_idx];

            assert_eq!(node.id, e_node.id);
            assert_eq!(node.func_id, func.id);
            assert_eq!(e_node.inputs.len(), node.inputs.len());
            assert_eq!(e_node.outputs.len(), func.outputs.len());

            for (input_idx, input) in node.inputs.iter().enumerate() {
                match &input.binding {
                    Binding::Output(output_binding) => {
                        if let Some(output_address) = &e_node.inputs[input_idx].output_address {
                            assert!(output_address.e_node_idx < self.e_nodes.len());

                            let output_e_node = &self.e_nodes[output_address.e_node_idx];
                            let output_node = &graph.nodes[output_e_node.node_idx];

                            assert_eq!(output_node.id, output_binding.output_node_id);
                            assert_eq!(output_e_node.id, output_binding.output_node_id);
                            assert!(output_address.port_idx < output_e_node.outputs.len());
                            assert_eq!(
                                output_e_node.outputs[output_address.port_idx],
                                ExecutionOutput::Used
                            );
                        }
                    }
                    Binding::None | Binding::Const => {
                        assert!(e_node.inputs[input_idx].output_address.is_none());
                    }
                }
            }
        }

        for idx in 0..self.e_node_execution_order.len() {
            let e_node_idx = self.e_node_execution_order[idx];
            assert!(e_node_idx < self.e_nodes.len());
            assert!(!self.e_node_execution_order[idx + 1..].contains(&e_node_idx));

            let e_node = &self.e_nodes[e_node_idx];
            assert!(!e_node.has_missing_inputs);

            let all_dependencies_in_order = e_node
                .inputs
                .iter()
                .filter_map(|input| input.output_address.as_ref())
                .all(|port_address| {
                    !self.e_node_execution_order[idx..].contains(&port_address.e_node_idx)
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
        let func = func_lib.by_id(node.func_id).unwrap();
        assert_eq!(node.inputs.len(), func.inputs.len());

        for input in node.inputs.iter() {
            if let Binding::Output(output_binding) = &input.binding {
                let output_node = graph.by_id(output_binding.output_node_id).unwrap();
                let output_func = func_lib.by_id(output_node.func_id).unwrap();
                assert!(output_binding.output_idx < output_func.outputs.len());
            }
        }
    }
}

fn convert_type(src_value: &DynamicValue, dst_data_type: &DataType) -> DynamicValue {
    let src_data_type = src_value.data_type();
    if *src_data_type == *dst_data_type {
        return src_value.clone();
    }

    if src_data_type.is_custom() || dst_data_type.is_custom() {
        panic!("Custom types are not supported yet");
    }

    match (src_data_type, dst_data_type) {
        (DataType::Bool, DataType::Int) => DynamicValue::Int(src_value.as_bool() as i64),
        (DataType::Bool, DataType::Float) => DynamicValue::Float(src_value.as_bool() as i64 as f64),
        (DataType::Bool, DataType::String) => DynamicValue::String(src_value.as_bool().to_string()),

        (DataType::Int, DataType::Bool) => DynamicValue::Bool(src_value.as_int() != 0),
        (DataType::Int, DataType::Float) => DynamicValue::Float(src_value.as_int() as f64),
        (DataType::Int, DataType::String) => DynamicValue::String(src_value.as_int().to_string()),

        (DataType::Float, DataType::Bool) => {
            DynamicValue::Bool(src_value.as_float().abs() > common::EPSILON)
        }
        (DataType::Float, DataType::Int) => DynamicValue::Int(src_value.as_float() as i64),
        (DataType::Float, DataType::String) => {
            DynamicValue::String(src_value.as_float().to_string())
        }

        (src, dst) => {
            panic!("Unsupported conversion from {:?} to {:?}", src, dst);
        }
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
        assert_eq!(execution_graph.e_node_execution_order.len(), 5);
        assert!(execution_graph
            .e_nodes
            .iter()
            .all(|e_node| !e_node.has_missing_inputs));

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

        assert!(!get_b.has_missing_inputs);
        assert!(sum.has_missing_inputs);
        assert!(mult.has_missing_inputs);
        assert!(print.has_missing_inputs);

        assert!(!get_b.has_changed_inputs);
        assert!(sum.has_changed_inputs);

        assert_eq!(sum.inputs[0].state, InputState::Missing);
        assert_eq!(mult.inputs[0].state, InputState::Missing);
        assert_eq!(print.inputs[0].state, InputState::Missing);

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
                binding: Binding::from_output_binding(graph.by_name("get_a").unwrap().id, 0),
                const_value: Some(StaticValue::Int(123)),
            },
            Input {
                binding: Binding::from_output_binding(graph.by_name("get_b").unwrap().id, 0),
                const_value: Some(StaticValue::Int(12)),
            },
        ];

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        assert_eq!(execution_graph.e_nodes.len(), 4);

        Ok(())
    }

    #[test]
    fn execution_graph_respects_removed_nodes() -> anyhow::Result<()> {
        // backward1() specifically crashed on this case

        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        graph.by_name_mut("mult").unwrap().inputs[0] = Input {
            binding: Binding::from_output_binding(graph.by_name("get_a").unwrap().id, 0),
            const_value: Some(StaticValue::Int(123)),
        };

        execution_graph.update(&graph, &func_lib)?;

        assert_eq!(execution_graph.e_nodes.len(), 4);

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
            .e_node_execution_order
            .iter()
            .any(|&idx| execution_graph.e_nodes[idx].name == "get_b"));

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib)?;

        // pure node not invoked if has cached values
        assert!(execution_graph
            .e_node_execution_order
            .iter()
            .all(|&idx| execution_graph.e_nodes[idx].name != "get_b"));

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
            .e_node_execution_order
            .iter()
            .any(|&idx| execution_graph.e_nodes[idx].name == "get_b"));

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
            .e_node_execution_order
            .iter()
            .any(|&idx| execution_graph.e_nodes[idx].name == "get_b"));

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);
        execution_graph.update(&graph, &func_lib)?;

        // once node not invoked is has cached outputs
        assert!(execution_graph
            .e_node_execution_order
            .iter()
            .all(|&idx| execution_graph.e_nodes[idx].name != "get_b"));

        Ok(())
    }

    #[test]
    fn cycle_detection_returns_error() {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mult_node_id = graph.by_name("mult").unwrap().id;
        let sum_inputs = &mut graph.by_name_mut("sum").unwrap().inputs;
        sum_inputs[0].binding = Binding::from_output_binding(mult_node_id, 0);

        let mut execution_graph = ExecutionGraph::default();
        let err = execution_graph
            .update(&graph, &func_lib)
            .expect_err("Expected cycle detection error");
        match err {
            ExecutionError::CycleDetected { node_id } => {
                assert_eq!(
                    node_id,
                    NodeId::from_str("579ae1d6-10a3-4906-8948-135cb7d7508b").unwrap()
                );
            }
            _ => panic!("Unexpected error"),
        }
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
            get_a: Box::new(move || test_values_a.try_lock().unwrap().a),
            get_b: Box::new(move || test_values_b.try_lock().unwrap().b),
            print: Box::new(move |result| {
                test_values_result.try_lock().unwrap().result = result;
            }),
        });

        let graph = test_graph();

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute(&graph, &func_lib)?;
        assert_eq!(test_values.try_lock()?.result, 35);

        // get_b is pure, so changing this should not affect result
        test_values.try_lock()?.b = 7;

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute(&graph, &func_lib)?;
        assert_eq!(test_values.try_lock()?.result, 35);

        func_lib.by_name_mut("get_b").unwrap().behavior = FuncBehavior::Impure;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute(&graph, &func_lib)?;

        assert_eq!(test_values.try_lock()?.result, 63);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn default_input_value() -> anyhow::Result<()> {
        let test_values = Arc::new(Mutex::new(TestValues {
            a: 2,
            b: 5,
            result: 0,
        }));
        let test_values_result = test_values.clone();

        let func_lib = test_func_lib(TestFuncHooks {
            print: Box::new(move |result| {
                test_values_result.try_lock().unwrap().result = result;
            }),
            ..TestFuncHooks::default()
        });

        let mut graph = test_graph();

        {
            let sum_inputs = &mut graph.by_name_mut("sum").unwrap().inputs;
            sum_inputs[0].const_value = Some(StaticValue::from(29));
            sum_inputs[0].binding = Binding::Const;
            sum_inputs[1].const_value = Some(StaticValue::from(11));
            sum_inputs[1].binding = Binding::Const;
        }

        {
            let mult_inputs = &mut graph.by_name_mut("mult").unwrap().inputs;
            mult_inputs[1].const_value = Some(StaticValue::from(9));
            mult_inputs[1].binding = Binding::Const;
        }

        let mut execution_graph = ExecutionGraph::default();

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute(&graph, &func_lib)?;

        assert_eq!(test_values.try_lock()?.result, 360);

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
            get_a: Box::new(move || {
                let mut guard = test_values_a.try_lock().unwrap();
                let a1 = guard.a;
                guard.a += 1;

                a1
            }),
            get_b: Box::new(move || {
                let mut guard = test_values_b.try_lock().unwrap();
                let b1 = guard.b;
                guard.b += 1;
                if b1 == 6 {
                    panic!("Unexpected call to get_b");
                }

                b1
            }),
            print: Box::new(move |result| {
                test_values_result.try_lock().unwrap().result = result;
            }),
        });

        let mut graph = test_graph();
        func_lib.by_name_mut("get_a").unwrap().behavior = FuncBehavior::Impure;
        graph.by_name_mut("get_a").unwrap().behavior = NodeBehavior::AsFunction;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute(&graph, &func_lib)?;

        // assert that both nodes were called
        {
            let guard = test_values.try_lock()?;
            assert_eq!(guard.a, 3);
            assert_eq!(guard.b, 6);
            assert_eq!(guard.result, 35);
        }

        execution_graph.update(&graph, &func_lib)?;
        execution_graph.execute(&graph, &func_lib)?;

        // assert that node was called again
        let guard = test_values.try_lock()?;
        assert_eq!(guard.a, 4);
        // but node b was cached
        assert_eq!(guard.b, 6);
        assert_eq!(guard.result, 40);

        Ok(())
    }
}
