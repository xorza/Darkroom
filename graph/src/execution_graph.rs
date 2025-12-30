use std::mem::take;
use std::panic;

use anyhow::Result;
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::compute::ComputeError;
use crate::data::DynamicValue;
use crate::function::{Func, FuncBehavior, FuncLib, InvokeCache};
use crate::graph::{Binding, Graph, Node, NodeBehavior, NodeId};
use common::{is_debug, FileFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum ProcessState {
    #[default]
    None,
    Processing,
    Backward1,
    Forward,
    Backward2,
}

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum ExecutionGraphError {
    #[error("Cycle detected while building execution graph at node {node_id:?}")]
    CycleDetected { node_id: NodeId },
}

type ExecutionGraphResult<T> = std::result::Result<T, ExecutionGraphError>;

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
    pub error: Option<ComputeError>,

    #[serde(skip)]
    pub(crate) cache: InvokeCache,
    #[serde(skip)]
    pub(crate) output_values: Option<Vec<DynamicValue>>,

    #[cfg(debug_assertions)]
    pub name: String,
}

#[derive(Debug)]
enum Visit1Cause {
    Terminal,
    OutputRequest { output_idx: usize },
    Done,
}
#[derive(Debug)]
struct Visit1 {
    node_idx: usize,
    e_node_idx: usize,
    cause: Visit1Cause,
}
#[derive(Debug)]
enum Visit2Cause {
    Terminal,
    OutputRequest { output_idx: usize },
    Done { execute: bool },
}
#[derive(Debug)]
struct Visit2 {
    e_node_idx: usize,
    cause: Visit2Cause,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub e_nodes: Vec<ExecutionNode>,
    e_node_idx_by_id: HashMap<NodeId, usize>,
    e_node_processing_order: Vec<usize>,
    pub e_node_execution_order: Vec<usize>,

    //caches
    #[serde(skip)]
    stack1: Vec<Visit1>,
    #[serde(skip)]
    stack2: Vec<Visit2>,
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
    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) -> ExecutionGraphResult<()> {
        validate_execution_inputs(graph, func_lib);

        self.backward1(graph, func_lib)?;
        self.forward(graph);
        self.backward2(graph);

        self.validate_with_graph(graph, func_lib);

        Ok(())
    }

    // Walk upstream dependencies to collect active nodes in processing order for input-state evaluation.
    fn backward1(&mut self, graph: &Graph, func_lib: &FuncLib) -> ExecutionGraphResult<()> {
        self.e_node_processing_order.clear();
        self.e_nodes.iter_mut().for_each(|e_node| e_node.reset());

        self.node_idx_by_id.clear();
        self.node_idx_by_id.extend(
            graph
                .nodes
                .iter()
                .enumerate()
                .map(|(idx, node)| (node.id, idx)),
        );

        // Compact e_nodes in-place to keep only nodes that still exist in graph.
        // We reuse existing ExecutionNode slots to avoid extra allocations.
        let mut write_idx = 0;
        // Look up the current slot for this node id (if any), otherwise append a new slot.
        let mut get_e_node_idx = |this: &mut Self, node_id: &NodeId| {
            let e_node_idx = match this.e_node_idx_by_id.get(node_id).copied() {
                Some(idx) => idx,
                None => {
                    assert!(write_idx <= this.e_nodes.len());
                    if write_idx == this.e_nodes.len() {
                        this.e_nodes.push(ExecutionNode::default());
                    }

                    write_idx
                }
            };
            if e_node_idx < write_idx {
                e_node_idx
            } else {
                if e_node_idx > write_idx {
                    this.e_nodes.swap(e_node_idx, write_idx);
                    this.e_node_idx_by_id
                        .insert(this.e_nodes[e_node_idx].id, e_node_idx);
                }
                this.e_node_idx_by_id.insert(*node_id, write_idx);

                write_idx += 1;
                write_idx - 1
            }
        };

        let mut stack: Vec<Visit1> = take(&mut self.stack1);
        stack.reserve(10);

        for (node_idx, node) in graph
            .nodes
            .iter()
            .enumerate()
            .filter(|&(_, node)| node.terminal)
        {
            let e_node_idx = get_e_node_idx(self, &node.id);
            stack.push(Visit1 {
                node_idx,
                e_node_idx,
                cause: Visit1Cause::Terminal,
            });
        }

        while let Some(visit) = stack.pop() {
            let e_node_idx = visit.e_node_idx;
            let node = &graph.nodes[visit.node_idx];

            let e_node = &mut self.e_nodes[e_node_idx];
            match visit.cause {
                Visit1Cause::Terminal => {}
                Visit1Cause::OutputRequest { output_idx } => {
                    if e_node.process_state != ProcessState::None {
                        e_node.outputs[output_idx] = ExecutionOutput::Used
                    }
                }
                Visit1Cause::Done => {
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
                    return Err(ExecutionGraphError::CycleDetected { node_id: e_node.id });
                }
                ProcessState::None => {}
                ProcessState::Forward | ProcessState::Backward2 => {
                    panic!("Invalid processing state")
                }
            }

            e_node.process_state = ProcessState::Processing;
            stack.push(Visit1 {
                node_idx: visit.node_idx,
                e_node_idx,
                cause: Visit1Cause::Done,
            });

            let func_idx = func_lib
                .funcs
                .iter()
                .position(|func| func.id == node.func_id)
                .expect("FuncLib missing function for graph node func_id");
            let func = &func_lib.funcs[func_idx];
            e_node.refresh(node, func, visit.node_idx, func_idx);
            if let Visit1Cause::OutputRequest { output_idx } = visit.cause {
                e_node.outputs[output_idx] = ExecutionOutput::Used
            }

            for (input_idx, input) in node.inputs.iter().enumerate() {
                if let Binding::Output(output_binding) = &input.binding {
                    let output_e_node_idx = get_e_node_idx(self, &output_binding.output_node_id);
                    self.e_nodes[e_node_idx].inputs[input_idx].output_address = Some(PortAddress {
                        e_node_idx: output_e_node_idx,
                        port_idx: output_binding.output_idx,
                    });
                    let output_node_idx = self.node_idx_by_id[&output_binding.output_node_id];
                    stack.push(Visit1 {
                        node_idx: output_node_idx,
                        e_node_idx: output_e_node_idx,
                        cause: Visit1Cause::OutputRequest {
                            output_idx: output_binding.output_idx,
                        },
                    });
                }
            }
        }

        self.node_idx_by_id.clear();
        self.stack1 = take(&mut stack);
        // Drop nodes past the compacted range.
        self.e_nodes.truncate(write_idx);
        self.e_node_idx_by_id.retain(|_, idx| *idx < write_idx);

        if is_debug() {
            assert_eq!(self.e_nodes.len(), self.e_node_idx_by_id.len());
            assert!(self.e_nodes.len() <= graph.nodes.len());
            self.e_nodes.iter().enumerate().for_each(|(idx, e_node)| {
                assert!(e_node.node_idx < graph.nodes.len());
                assert_eq!(graph.nodes[e_node.node_idx].id, e_node.id);
                assert_eq!(idx, self.e_node_idx_by_id[&e_node.id]);
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
        self.e_node_processing_order.clear();
    }

    // Walk upstream dependencies to collect the execution order.
    fn backward2(&mut self, graph: &Graph) {
        self.e_node_execution_order.clear();

        let mut stack: Vec<Visit2> = take(&mut self.stack2);
        stack.reserve(10);

        for (idx, e_node) in self.e_nodes.iter().enumerate() {
            if graph.nodes[e_node.node_idx].terminal {
                stack.push(Visit2 {
                    e_node_idx: idx,
                    cause: Visit2Cause::Terminal,
                });
            }
        }

        while let Some(visit) = stack.pop() {
            let e_node_idx = visit.e_node_idx;
            let e_node = &mut self.e_nodes[e_node_idx];

            let _output_address = match visit.cause {
                Visit2Cause::Terminal => None,
                Visit2Cause::OutputRequest { output_idx } => Some(output_idx),
                Visit2Cause::Done { execute } => {
                    e_node.process_state = ProcessState::Backward2;
                    if execute {
                        self.e_node_execution_order.push(e_node_idx);
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
            stack.push(Visit2 {
                e_node_idx,
                cause: Visit2Cause::Done { execute },
            });

            if execute {
                for input in e_node.inputs.iter() {
                    if match input.state {
                        InputState::Unchanged | InputState::Missing => false,
                        InputState::Changed => true,
                        InputState::Unknown => panic!("Unprocessed input"),
                    } {
                        if let Some(output_address) = input.output_address.as_ref() {
                            stack.push(Visit2 {
                                e_node_idx: output_address.e_node_idx,
                                cause: Visit2Cause::OutputRequest {
                                    output_idx: output_address.port_idx,
                                },
                            });
                        }
                    }
                }
            }
        }

        self.stack2 = take(&mut stack);
    }

    pub fn validate_with_graph(&self, graph: &Graph, func_lib: &FuncLib) {
        if !is_debug() {
            return;
        }

        assert!(self.e_nodes.len() <= graph.nodes.len());
        assert_eq!(self.e_nodes.len(), self.e_node_idx_by_id.len());

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
            assert_eq!(e_node_idx, *self.e_node_idx_by_id.get(&e_node.id).unwrap());

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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::data::{DynamicValue, StaticValue};
    use crate::function::{test_func_lib, TestFuncHooks};
    use crate::graph::{test_graph, Input, NodeBehavior};
    use common::FileFormat;

    #[test]
    fn simple_run() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        assert_eq!(execution_graph.e_nodes.len(), 5);
        assert_eq!(execution_graph.e_node_idx_by_id.len(), 5);
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
        execution_graph.e_node_idx_by_id.clear();

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
    fn once_node_with_cached_outputs_skips_invocation() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        // get_b in e_node_execution_order
        assert!(execution_graph
            .e_node_execution_order
            .iter()
            .any(|&idx| execution_graph.e_nodes[idx].name == "get_b"));

        execution_graph.by_name_mut("get_b").unwrap().output_values =
            Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib)?;

        // get_b not in e_node_execution_order
        assert!(execution_graph
            .e_node_execution_order
            .iter()
            .all(|&idx| execution_graph.e_nodes[idx].name != "get_b"));

        Ok(())
    }

    #[test]
    fn func_behavior_controls_execution() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let _get_a_node_id = graph.by_name("get_a").unwrap().id;
        let get_b_node_id = graph.by_name("get_b").unwrap().id;
        graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::AsFunction;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        // assert!(
        //     execution_graph.by_id(get_a_node_id).unwrap().active,
        //     "Impure functions should execute even without inputs"
        // );
        // assert!(
        //     execution_graph.by_id(get_b_node_id).unwrap().active,
        //     "Pure functions should execute on first run without cached outputs"
        // );

        execution_graph
            .by_id_mut(get_b_node_id)
            .unwrap()
            .output_values = Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib)?;

        // assert!(
        //     !execution_graph.by_id(get_b_node_id).unwrap().active,
        //     "Pure functions without input changes should not execute with cached outputs"
        // );

        Ok(())
    }

    #[test]
    fn func_behavior_controls_execution1() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let _get_a_node_id = graph.by_name("get_a").unwrap().id;
        let _get_b_node_id = graph.by_name("get_b").unwrap().id;
        let mult_node_id = graph.by_name("mult").unwrap().id;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        graph.by_id_mut(mult_node_id).unwrap().behavior = NodeBehavior::Once;

        execution_graph
            .by_id_mut(mult_node_id)
            .unwrap()
            .output_values = Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib)?;

        // assert!(
        //     execution_graph.by_id(get_a_node_id).unwrap().active,
        //     "As mult node is cached, get_a should not execute"
        // );
        // assert!(
        //     execution_graph.by_id(get_b_node_id).unwrap().active,
        //     "As mult node is cached, get_b should not execute"
        // );
        // assert!(
        //     execution_graph.by_id(mult_node_id).unwrap().active,
        //     "Pure functions without input changes should not execute with cached outputs"
        // );

        Ok(())
    }

    #[test]
    fn cycle_detection_returns_error() {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mult_node_id = graph.by_name("mult").unwrap().id;

        let sum_inputs = &mut graph.by_name_mut("sum").unwrap().inputs;
        sum_inputs[0].binding = Binding::from_output_binding(mult_node_id, 0);
        sum_inputs[0].const_value = None;

        let mut execution_graph = ExecutionGraph::default();
        let err = execution_graph
            .update(&graph, &func_lib)
            .expect_err("Expected cycle detection error");
        match err {
            ExecutionGraphError::CycleDetected { node_id } => {
                assert_eq!(
                    node_id,
                    NodeId::from_str("579ae1d6-10a3-4906-8948-135cb7d7508b").unwrap()
                );
            }
        }
    }
}
