use std::mem::take;
use std::thread::panicking;

use anyhow::Result;
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::compute::ComputeError;
use crate::data::DynamicValue;
use crate::function::{Func, FuncLib, InvokeCache};
use crate::graph::{Binding, Graph, Node, NodeBehavior, NodeId};
use crate::prelude::FuncBehavior;
use common::{deserialize, serialize, FileFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum ProcessingState {
    #[default]
    None,
    Processing,
    Processed1,
    Processed2,
}

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum ExecutionGraphError {
    #[error("Cycle detected while building execution graph at node {node_id:?}")]
    CycleDetected { node_id: NodeId },
}

type ExecutionGraphResult<T> = std::result::Result<T, ExecutionGraphError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ExecutionNode {
    pub id: NodeId,

    pub has_missing_inputs: bool,
    pub has_changed_inputs: bool,
    pub should_invoke: bool,
    pub function_behavior: FuncBehavior,

    pub node_idx: usize,
    pub func_idx: usize,
    pub inputs: Vec<ExecutionInput>,
    pub outputs: Vec<ExecutionOutput>,

    processing_state: ProcessingState,

    pub run_time: f64,
    pub error: Option<ComputeError>,

    #[serde(skip)]
    pub(crate) cache: InvokeCache,
    #[serde(skip)]
    pub(crate) output_values: Option<Vec<DynamicValue>>,

    #[cfg(debug_assertions)]
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExecutionGraph {
    pub e_nodes: Vec<ExecutionNode>,
    e_node_idx_by_id: HashMap<NodeId, usize>,
    pub e_node_processing_order: Vec<usize>,
    pub e_node_execution_order: Vec<usize>,
}

impl ExecutionNode {
    fn reset_from(&mut self, node: &Node) {
        let mut prev_state = take(self);

        self.id = node.id;

        self.inputs = take(&mut prev_state.inputs);
        self.inputs.fill(ExecutionInput::default());
        self.inputs
            .resize(node.inputs.len(), ExecutionInput::default());

        self.outputs = take(&mut prev_state.outputs);
        self.outputs.fill(ExecutionOutput::default());

        #[cfg(debug_assertions)]
        {
            self.name = take(&mut prev_state.name);
            self.name.clear();
            self.name.push_str(&node.name);
        }

        self.cache = take(&mut prev_state.cache);
        self.output_values = take(&mut prev_state.output_values);
    }

    fn reset_ports_from_func(&mut self, func: &Func) {
        self.inputs.clear();
        self.inputs.reserve(func.inputs.len());
        for input in &func.inputs {
            self.inputs.push(ExecutionInput {
                required: input.required,
                ..Default::default()
            });
        }

        self.outputs.fill(ExecutionOutput::Unused);
        self.outputs
            .resize(func.outputs.len(), ExecutionOutput::default());
    }
}

impl ExecutionGraph {
    pub fn by_id(&self, node_id: NodeId) -> Option<&ExecutionNode> {
        self.e_nodes.iter().find(|node| node.id == node_id)
    }
    pub fn by_id_mut(&mut self, node_id: NodeId) -> Option<&mut ExecutionNode> {
        self.e_nodes.iter_mut().find(|node| node.id == node_id)
    }

    pub fn serialize(&self, format: FileFormat) -> String {
        serialize(self, format)
    }

    pub fn deserialize(serialized: &str, format: FileFormat) -> anyhow::Result<Self> {
        let execution_graph: ExecutionGraph = deserialize(serialized, format)?;

        Ok(execution_graph)
    }

    // Rebuild execution state from the current graph and function library.
    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) -> ExecutionGraphResult<()> {
        self.update_node_cache(graph);
        self.backward(graph, func_lib)?;
        self.forward(graph);

        #[cfg(debug_assertions)]
        self.validate_with_graph(graph);

        Ok(())
    }

    // Update the node cache with the current graph.
    fn update_node_cache(&mut self, graph: &Graph) {
        // Compact e_nodes in-place to keep only nodes that still exist in graph.
        // We reuse existing ExecutionNode slots to avoid extra allocations.
        let mut write_idx = 0;
        for (node_idx, node) in graph.nodes.iter().enumerate() {
            // Look up the current slot for this node id (if any), otherwise append a new slot.
            let e_node_idx = match self.e_node_idx_by_id.get(&node.id).copied() {
                Some(idx) => idx,
                None => {
                    if write_idx >= self.e_nodes.len() {
                        self.e_nodes.push(ExecutionNode::default());
                    }
                    write_idx
                }
            };

            // Move the execution node we want into the next compacted slot.
            if e_node_idx != write_idx {
                self.e_nodes.swap(e_node_idx, write_idx);
                let swapped_id = self.e_nodes[e_node_idx].id;
                if !swapped_id.is_nil() {
                    // The swapped node moved; update its cached index.
                    self.e_node_idx_by_id.insert(swapped_id, e_node_idx);
                }
            }

            // Reset the execution node with the latest graph node data.
            let e_node = &mut self.e_nodes[write_idx];
            e_node.reset_from(node);
            e_node.node_idx = node_idx;
            self.e_node_idx_by_id.insert(node.id, write_idx);
            write_idx += 1;
        }

        // Drop any execution nodes past the compacted range.
        self.e_nodes.truncate(write_idx);
        // Prune stale id->index entries that point past the new length or mismatched ids.
        self.e_node_idx_by_id
            .retain(|id, idx| *idx < self.e_nodes.len() && self.e_nodes[*idx].id == *id);

        #[cfg(debug_assertions)]
        {
            assert_eq!(
                self.e_nodes.len(),
                self.e_node_idx_by_id.len(),
                "Execution node count mismatch"
            );
            assert_eq!(
                self.e_nodes.len(),
                graph.nodes.len(),
                "Execution node count mismatch"
            );
            // Check that the execution graph is in a consistent state.
            self.e_nodes.iter().enumerate().for_each(|(idx, e_node)| {
                assert_eq!(
                    idx, self.e_node_idx_by_id[&e_node.id],
                    "Execution node index mismatch"
                );
                assert!(
                    e_node.node_idx < graph.nodes.len(),
                    "Execution node index out of bounds"
                );
                assert_eq!(
                    graph.nodes[e_node.node_idx].id, e_node.id,
                    "Execution node id mismatch"
                );
            });
        }
    }

    // Walk upstream dependencies to mark active nodes and compute invocation order.
    fn backward(&mut self, graph: &Graph, func_lib: &FuncLib) -> ExecutionGraphResult<()> {
        enum VisitCause {
            Terminal,
            OutputRequest { output_address: PortAddress },
            Processed,
        }
        struct Visit {
            e_node_idx: usize,
            cause: VisitCause,
        }
        let mut stack: Vec<Visit> = Vec::with_capacity(10);

        self.e_node_processing_order.clear();
        for (idx, e_node) in self.e_nodes.iter().enumerate() {
            if graph.nodes[e_node.node_idx].terminal {
                stack.push(Visit {
                    e_node_idx: idx,
                    cause: VisitCause::Terminal,
                });
            }
        }

        while let Some(visit) = stack.pop() {
            let e_node_idx = visit.e_node_idx;
            let node_idx = {
                let e_node = &mut self.e_nodes[e_node_idx];

                let output_address = match visit.cause {
                    VisitCause::Terminal => None,
                    VisitCause::OutputRequest { output_address } => Some(output_address),
                    VisitCause::Processed => {
                        e_node.processing_state = ProcessingState::Processed1;
                        self.e_node_processing_order.push(e_node_idx);
                        continue;
                    }
                };

                match e_node.processing_state {
                    ProcessingState::Processed1 => {
                        continue;
                    }
                    ProcessingState::Processing => {
                        return Err(ExecutionGraphError::CycleDetected { node_id: e_node.id });
                    }
                    ProcessingState::None => {
                        let func_idx = func_lib
                            .funcs
                            .iter()
                            .position(|func| func.id == graph.nodes[e_node.node_idx].func_id)
                            .expect("FuncLib missing function for graph node func_id");
                        let func = &func_lib.funcs[func_idx];
                        e_node.reset_ports_from_func(func);
                        e_node.func_idx = func_idx;
                        e_node.function_behavior = func.behavior;
                    }
                    ProcessingState::Processed2 => panic!("Unexpected state"),
                }

                if let Some(output_address) = output_address {
                    e_node.outputs[output_address.port_idx] = ExecutionOutput::Used;
                }
                e_node.processing_state = ProcessingState::Processing;
                stack.push(Visit {
                    e_node_idx,
                    cause: VisitCause::Processed,
                });

                e_node.node_idx
            };

            for (input_idx, input) in graph.nodes[node_idx].inputs.iter().enumerate() {
                if let Binding::Output(output_binding) = &input.binding {
                    let output_e_node_idx = self.e_node_idx_by_id[&output_binding.output_node_id];
                    self.e_nodes[e_node_idx].inputs[input_idx].output_address = Some(PortAddress {
                        e_node_idx: output_e_node_idx,
                        port_idx: output_binding.output_idx,
                    });
                    stack.push(Visit {
                        e_node_idx: output_e_node_idx,
                        cause: VisitCause::OutputRequest {
                            output_address: PortAddress {
                                e_node_idx: output_e_node_idx,
                                port_idx: output_binding.output_idx,
                            },
                        },
                    });
                }
            }
        }
        Ok(())
    }

    // Propagate input state forward from scheduled nodes to set invoke/missing flags.
    fn forward(&mut self, graph: &Graph) {
        self.e_node_execution_order.clear();

        for e_node_idx in self.e_node_processing_order.iter().copied() {
            let node = {
                let e_node = &mut self.e_nodes[e_node_idx];
                assert_eq!(e_node.processing_state, ProcessingState::Processed1);

                let node = &graph.nodes[e_node.node_idx];
                // avoid traversing inputs for NodeBehavior::Once nodes having outputs
                // even if having missing inputs
                if node.behavior == NodeBehavior::CacheOutput && e_node.output_values.is_some() {
                    // should_invoke is false
                    continue;
                }

                node
            };

            let mut has_changed_inputs = false;
            let mut has_missing_inputs = false;

            for (input_idx, input) in node.inputs.iter().enumerate() {
                let input_state = match &input.binding {
                    Binding::None => InputState::Missing,
                    // Const bindings are treated as changed each run until change tracking exists.
                    Binding::Const => InputState::Changed,
                    Binding::Output(_) => {
                        let output_e_node_idx = self.e_nodes[e_node_idx].inputs[input_idx]
                            .output_address
                            .expect("Output binding references missing execution node")
                            .e_node_idx;
                        let output_e_node = &self.e_nodes[output_e_node_idx];

                        assert!(
                            output_e_node.processing_state == ProcessingState::Processed1
                                || output_e_node.processing_state == ProcessingState::Processed2
                        );

                        if output_e_node.has_missing_inputs {
                            InputState::Missing
                        } else if output_e_node.should_invoke {
                            InputState::Changed
                        } else {
                            InputState::Unchanged
                        }
                    }
                };
                match input_state {
                    InputState::Unchanged => {}
                    InputState::Changed => has_changed_inputs = true,
                    InputState::Missing => {
                        has_missing_inputs |= self.e_nodes[e_node_idx].inputs[input_idx].required;
                    }
                    InputState::Unknown => panic!("unprocessed input"),
                }

                self.e_nodes[e_node_idx].inputs[input_idx].state = input_state;
            }

            let e_node = &mut self.e_nodes[e_node_idx];

            let should_invoke = if has_missing_inputs {
                false
            } else {
                match node.behavior {
                    NodeBehavior::CacheOutput => {
                        // has no cached outputs, so should_invoke = true
                        true
                    }
                    NodeBehavior::AsFunction => match e_node.function_behavior {
                        FuncBehavior::Impure => true,
                        FuncBehavior::Pure => has_changed_inputs || e_node.output_values.is_none(),
                        FuncBehavior::Output => {
                            panic!("For Output functions, node should have Terminal behavior")
                        }
                    },
                }
            };

            e_node.has_missing_inputs = has_missing_inputs;
            e_node.processing_state = ProcessingState::Processed2;
            e_node.should_invoke = should_invoke;

            if should_invoke {
                self.e_node_execution_order.push(e_node_idx);
            }
        }
    }

    pub fn validate_with_graph(&self, graph: &Graph) {
        #[cfg(not(debug_assertions))]
        tracing::warn!("Running validate_with_graph in release mode. May be suboptimal.");

        assert_eq!(
            self.e_nodes.len(),
            graph.nodes.len(),
            "Execution node count mismatch"
        );
        assert_eq!(
            self.e_nodes.len(),
            self.e_node_idx_by_id.len(),
            "Execution node index map mismatch"
        );

        let mut seen_node_indices = vec![false; graph.nodes.len()];
        for (e_node_idx, e_node) in self.e_nodes.iter().enumerate() {
            assert!(
                e_node.node_idx < graph.nodes.len(),
                "Execution node index out of bounds"
            );
            let node = &graph.nodes[e_node.node_idx];
            assert_eq!(
                node.id, e_node.id,
                "Execution node id mismatch for graph node {}",
                e_node.node_idx
            );
            assert!(
                !seen_node_indices[e_node.node_idx],
                "Duplicate execution node for graph node {}",
                e_node.node_idx
            );
            seen_node_indices[e_node.node_idx] = true;

            let mapped_idx = self
                .e_node_idx_by_id
                .get(&e_node.id)
                .expect("Execution node id missing from index map");
            assert_eq!(
                *mapped_idx, e_node_idx,
                "Execution node index map mismatch for node {:?}",
                e_node.id
            );

            assert_eq!(
                e_node.inputs.len(),
                node.inputs.len(),
                "Execution node input count mismatch for node {:?}",
                e_node.id
            );

            for (input_idx, input) in node.inputs.iter().enumerate() {
                match &input.binding {
                    Binding::Output(output_binding) => {
                        if let Some(address) = e_node.inputs[input_idx].output_address {
                            assert!(
                                address.e_node_idx < self.e_nodes.len(),
                                "Execution output address node index out of bounds"
                            );
                            let output_node = &self.e_nodes[address.e_node_idx];
                            assert_eq!(
                                output_node.id, output_binding.output_node_id,
                                "Execution output address points at wrong node"
                            );
                            assert!(
                                address.port_idx < output_node.outputs.len(),
                                "Execution output address port index out of bounds"
                            );
                        }
                    }
                    Binding::None | Binding::Const => {
                        assert!(
                            e_node.inputs[input_idx].output_address.is_none(),
                            "Non-output binding should not have an execution output address"
                        );
                    }
                }
            }
        }
        assert!(
            seen_node_indices.iter().all(|seen| *seen),
            "Execution graph is missing nodes from the source graph"
        );

        let mut in_processing_order = vec![false; self.e_nodes.len()];
        for &e_node_idx in self.e_node_processing_order.iter() {
            assert!(
                e_node_idx < self.e_nodes.len(),
                "Execution node processing index out of bounds"
            );
            assert!(
                !in_processing_order[e_node_idx],
                "Duplicate execution node in processing order"
            );
            in_processing_order[e_node_idx] = true;
            assert!(
                self.e_nodes[e_node_idx].processing_state == ProcessingState::Processed1
                    || self.e_nodes[e_node_idx].processing_state == ProcessingState::Processed2,
                "Execution node {} in processing order is not processed",
                e_node_idx
            );
        }
        for (idx, e_node) in self.e_nodes.iter().enumerate() {
            assert_ne!(
                e_node.processing_state,
                ProcessingState::Processing,
                "Execution node {} is still processing",
                idx
            );
            if !in_processing_order[idx] {
                assert_ne!(
                    e_node.processing_state,
                    ProcessingState::Processed1,
                    "Execution node {} is processed but missing from processing order",
                    idx
                );
            }
        }

        let mut in_execution_order = vec![false; self.e_nodes.len()];
        for &e_node_idx in self.e_node_execution_order.iter() {
            assert!(
                e_node_idx < self.e_nodes.len(),
                "Execution node execution index out of bounds"
            );
            assert!(
                !in_execution_order[e_node_idx],
                "Duplicate execution node in execution order"
            );
            in_execution_order[e_node_idx] = true;
            assert!(
                self.e_nodes[e_node_idx].should_invoke,
                "Execution node {} in execution order should_invoke=false",
                e_node_idx
            );
            assert_eq!(
                self.e_nodes[e_node_idx].processing_state,
                ProcessingState::Processed2,
                "Execution node {} in execution order is not processed",
                e_node_idx
            );
        }
        for (idx, e_node) in self.e_nodes.iter().enumerate() {
            if e_node.should_invoke {
                assert!(
                    in_execution_order[idx],
                    "Execution node {} should invoke but is missing from execution order",
                    idx
                );
            } else {
                assert!(
                    !in_execution_order[idx],
                    "Execution node {} should not invoke but is in execution order",
                    idx
                );
            }
        }
    }
}

fn validate_execution_inputs(graph: &Graph, func_lib: &FuncLib) -> Result<()> {
    graph.validate()?;

    for node in graph.nodes.iter() {
        let func = func_lib.by_id(node.func_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "Missing function {:?} for node {:?}",
                node.func_id, node.id
            ))
        })?;

        if node.inputs.len() != func.inputs.len() {
            return Err(anyhow::Error::msg(format!(
                "Node {:?} input count mismatch",
                node.id
            )));
        }

        for input in node.inputs.iter() {
            if let Binding::Output(output_binding) = &input.binding {
                let output_node = graph
                    .by_id(output_binding.output_node_id)
                    .ok_or_else(|| anyhow::Error::msg("Output binding references missing node"))?;
                let output_func = func_lib.by_id(output_node.func_id).ok_or_else(|| {
                    anyhow::Error::msg(format!(
                        "Missing function {:?} for output node {:?}",
                        output_node.func_id, output_node.id
                    ))
                })?;
                if output_binding.output_idx >= output_func.outputs.len() {
                    return Err(anyhow::Error::msg(format!(
                        "Output index out of range for node {:?}",
                        output_binding.output_node_id
                    )));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::data::{DynamicValue, StaticValue};
    use crate::function::{test_func_lib, FuncBehavior, TestFuncHooks};
    use crate::graph::{test_graph, Input, NodeBehavior};
    use common::FileFormat;

    #[test]
    fn simple_run() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let _get_b_node_id = graph.by_name("get_b").unwrap().id;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        assert_eq!(execution_graph.e_nodes.len(), 5);
        assert!(execution_graph
            .e_nodes
            .iter()
            .all(|e_node| e_node.should_invoke));
        assert!(execution_graph
            .e_nodes
            .iter()
            .all(|e_node| !e_node.has_missing_inputs));

        Ok(())
    }

    #[test]
    fn empty_run() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let _get_b_node_id = graph
            .by_name("get_b")
            .expect("Node named \"get_b\" not found")
            .id;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        assert_eq!(execution_graph.e_nodes.len(), 5);

        let execution_order_before: Vec<usize> = execution_graph.e_node_processing_order.clone();

        execution_graph.update(&graph, &func_lib)?;

        assert_eq!(execution_graph.e_nodes.len(), 5);
        assert!(
            execution_graph
                .e_nodes
                .iter()
                .all(|e_node| e_node.processing_state == ProcessingState::Processed2),
            "Execution nodes should be processed after update"
        );
        let execution_order_after: Vec<usize> = execution_graph.e_node_processing_order.clone();
        assert_eq!(
            execution_order_before, execution_order_after,
            "Invocation order should remain stable across updates"
        );

        Ok(())
    }

    #[test]
    fn missing_input() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let get_b_node_id = graph.by_name("get_b").unwrap().id;
        let sum_node_id = graph.by_name("sum").unwrap().id;
        let mult_node_id = graph.by_name("mult").unwrap().id;
        let print_node_id = graph.by_name("print").unwrap().id;

        graph.by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        assert_eq!(execution_graph.e_nodes.len(), 5);
        assert!(execution_graph.by_id(get_b_node_id).unwrap().should_invoke);
        assert!(!execution_graph.by_id(sum_node_id).unwrap().should_invoke);
        assert!(!execution_graph.by_id(mult_node_id).unwrap().should_invoke);
        assert!(!execution_graph.by_id(print_node_id).unwrap().should_invoke);
        assert!(
            !execution_graph
                .by_id(get_b_node_id)
                .unwrap()
                .has_missing_inputs
        );
        assert!(
            execution_graph
                .by_id(sum_node_id)
                .unwrap()
                .has_missing_inputs
        );
        assert!(
            execution_graph
                .by_id(mult_node_id)
                .unwrap()
                .has_missing_inputs
        );
        assert!(
            execution_graph
                .by_id(print_node_id)
                .unwrap()
                .has_missing_inputs
        );

        let sum_node = execution_graph.by_id(sum_node_id).unwrap();
        assert_eq!(sum_node.inputs[0].state, InputState::Missing);

        let mult_node = execution_graph.by_id(mult_node_id).unwrap();
        assert_eq!(mult_node.inputs[0].state, InputState::Missing);

        let print_node = execution_graph.by_id(print_node_id).unwrap();
        assert_eq!(print_node.inputs[0].state, InputState::Missing);

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

        let get_a_node_id = graph.by_name("get_a").unwrap().id;
        let get_b_node_id = graph.by_name("get_b").unwrap().id;

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

        let sum_node_id = graph.by_name("sum").unwrap().id;
        graph.remove_by_id(sum_node_id);

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;
        execution_graph.validate_with_graph(&graph);

        assert_eq!(graph.nodes.len(), 4);
        assert_eq!(execution_graph.e_nodes.len(), 4);

        let mult_node_id = graph.by_name("mult").unwrap().id;
        let mult_node = execution_graph.by_id(mult_node_id).unwrap();
        let mult_input_a = mult_node.inputs[0].output_address.unwrap();
        let mult_input_b = mult_node.inputs[1].output_address.unwrap();
        assert_eq!(
            execution_graph.e_nodes[mult_input_a.e_node_idx].id, get_a_node_id,
            "Mult input A should be wired to get_a"
        );
        assert_eq!(
            execution_graph.e_nodes[mult_input_b.e_node_idx].id, get_b_node_id,
            "Mult input B should be wired to get_b"
        );
        Ok(())
    }

    #[test]
    fn once_node_with_cached_outputs_skips_invocation() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        let get_b_node_id = graph.by_name("get_b").unwrap().id;
        execution_graph
            .by_id_mut(get_b_node_id)
            .unwrap()
            .output_values = Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib)?;

        let get_b_node = execution_graph.by_id(get_b_node_id).unwrap();
        assert!(
            !get_b_node.should_invoke,
            "Once node with cached outputs should not invoke"
        );
        Ok(())
    }

    #[test]
    fn func_behavior_controls_execution() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib(TestFuncHooks::default());

        let get_a_node_id = graph.by_name("get_a").unwrap().id;
        let get_b_node_id = graph.by_name("get_b").unwrap().id;
        graph.by_name_mut("get_b").unwrap().behavior = NodeBehavior::AsFunction;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib)?;

        assert!(
            execution_graph.by_id(get_a_node_id).unwrap().should_invoke,
            "Impure functions should execute even without inputs"
        );
        assert!(
            execution_graph.by_id(get_b_node_id).unwrap().should_invoke,
            "Pure functions should execute on first run without cached outputs"
        );

        execution_graph
            .by_id_mut(get_b_node_id)
            .unwrap()
            .output_values = Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib)?;

        assert!(
            !execution_graph.by_id(get_b_node_id).unwrap().should_invoke,
            "Pure functions without input changes should not execute with cached outputs"
        );

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
