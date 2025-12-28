use std::mem::take;

use anyhow::Result;
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::common::FileFormat;
use crate::data::DynamicValue;
use crate::function::{Func, FuncLib};
use crate::graph::{Binding, Graph, Node, NodeBehavior, NodeId};
use crate::invoke::InvokeCache;
use common::normalize_string::NormalizeString;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum ProcessingState {
    #[default]
    None,
    Processing,
    Processed,
}

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
    pub invocation_order: usize,

    pub node_idx: usize,
    pub func_idx: usize,
    pub inputs: Vec<ExecutionInput>,
    pub outputs: Vec<ExecutionOutput>,

    processing_state: ProcessingState,

    pub run_time: f64,

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
        match format {
            FileFormat::Yaml => serde_yml::to_string(&self)
                .expect("Failed to serialize execution graph to YAML")
                .normalize(),
            FileFormat::Json => serde_json::to_string_pretty(&self)
                .expect("Failed to serialize execution graph to JSON")
                .normalize(),
        }
    }

    pub fn deserialize(serialized: &str, format: FileFormat) -> anyhow::Result<Self> {
        let execution_graph: ExecutionGraph = match format {
            FileFormat::Yaml => serde_yml::from_str(serialized)?,
            FileFormat::Json => serde_json::from_str(serialized)?,
        };

        Ok(execution_graph)
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
            let graph_node = &graph.nodes[e_node.node_idx];
            assert_eq!(
                graph_node.id, e_node.id,
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
                graph_node.inputs.len(),
                "Execution node input count mismatch for node {:?}",
                e_node.id
            );

            for (input_idx, input) in graph_node.inputs.iter().enumerate() {
                match &input.binding {
                    Binding::Output(output_binding) => {
                        let address = e_node.inputs[input_idx]
                            .output_address
                            .expect("Output binding missing execution output address");
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
    }

    // Rebuild execution state from the current graph and function library.
    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) {
        self.update_node_cache(graph);
        self.backward(graph, func_lib);
        self.forward(graph);

        #[cfg(debug_assertions)]
        self.validate_with_graph(graph);
    }

    // Update the node cache with the current graph.
    fn update_node_cache(&mut self, graph: &Graph) {
        // Compact e_nodes in-place to keep only nodes that still exist in graph.
        // We reuse existing ExecutionNode slots to avoid extra allocations.
        let mut write_idx = 0;
        for (node_idx, node) in graph.nodes.iter().enumerate() {
            assert!(
                !node.id.is_nil(),
                "Graph node has nil id at index {}",
                node_idx
            );

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
    fn backward(&mut self, graph: &Graph, func_lib: &FuncLib) {
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

        self.e_nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, e_node)| {
                if graph.nodes[e_node.node_idx].behavior == NodeBehavior::Terminal {
                    Some(idx)
                } else {
                    None
                }
            })
            .for_each(|idx| {
                stack.push(Visit {
                    e_node_idx: idx,
                    cause: VisitCause::Terminal,
                });
            });

        let mut invocation_order: usize = 0;
        while let Some(visit) = stack.pop() {
            let e_node_idx = visit.e_node_idx;
            let node_idx = {
                let e_node = &mut self.e_nodes[e_node_idx];

                let output_address = match visit.cause {
                    VisitCause::Terminal => None,
                    VisitCause::OutputRequest { output_address } => {
                        assert_eq!(
                            visit.e_node_idx, output_address.e_node_idx,
                            "Visit e_node_idx {} does not match output address e_node_idx {}",
                            visit.e_node_idx, output_address.e_node_idx
                        );
                        Some(output_address)
                    }
                    VisitCause::Processed => {
                        e_node.processing_state = ProcessingState::Processed;
                        e_node.invocation_order = invocation_order;
                        invocation_order += 1;
                        continue;
                    }
                };

                match e_node.processing_state {
                    ProcessingState::Processed => {
                        continue;
                    }
                    ProcessingState::Processing => {
                        // todo replace with result<>
                        panic!(
                            "Cycle detected while building execution graph at node {:?}",
                            visit.e_node_idx
                        );
                    }
                    ProcessingState::None => {
                        let func_idx = func_lib
                            .funcs
                            .iter()
                            .position(|func| func.id == graph.nodes[e_node.node_idx].func_id)
                            .expect("FuncLib missing function for graph node func_id");
                        e_node.reset_ports_from_func(&func_lib.funcs[func_idx]);
                        e_node.func_idx = func_idx;
                    }
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
    }

    // Propagate input state forward from scheduled nodes to set invoke/missing flags.
    fn forward(&mut self, graph: &Graph) {
        let mut active_e_node_indices = self
            .e_nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, e_node)| {
                (e_node.processing_state == ProcessingState::Processed).then_some(idx)
            })
            .collect::<Vec<_>>();
        active_e_node_indices.sort_by_key(|&idx| self.e_nodes[idx].invocation_order);

        for e_node_idx in active_e_node_indices {
            let node = {
                let e_node = &mut self.e_nodes[e_node_idx];
                assert_eq!(
                    e_node.processing_state,
                    ProcessingState::Processed,
                    "Execution node must be processed before input propagation"
                );

                let node = &graph.nodes[e_node.node_idx];
                // avoid traversing inputs for NodeBehavior::Once nodes having outputs
                // even if having missing inputs
                if node.behavior == NodeBehavior::Once && e_node.output_values.is_some() {
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
                    Binding::Output(output_binding) => {
                        let output_e_node_idx = self.e_nodes[e_node_idx].inputs[input_idx]
                            .output_address
                            .expect("Output binding references missing execution node")
                            .e_node_idx;
                        let output_e_node = &self.e_nodes[output_e_node_idx];

                        assert_eq!(
                            output_e_node.processing_state,
                            ProcessingState::Processed,
                            "Output execution node {:?} not processed before input propagation",
                            output_binding.output_node_id
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
            e_node.has_missing_inputs = has_missing_inputs;
            if !e_node.has_missing_inputs {
                match node.behavior {
                    NodeBehavior::Terminal | NodeBehavior::Always => {
                        e_node.should_invoke = true;
                    }
                    NodeBehavior::Once => {
                        // has no cached outputs, so should_invoke = true
                        e_node.should_invoke = true;
                    }
                    NodeBehavior::OnInputChange => {
                        e_node.should_invoke = has_changed_inputs;
                    }
                }
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
    use super::*;
    use crate::common::FileFormat;
    use crate::data::{DynamicValue, StaticValue};
    use crate::function::test_func_lib;
    use crate::graph::{test_graph, Input};

    #[test]
    fn simple_run() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib();

        let _get_b_node_id = graph
            .by_name("get_b")
            .expect("Node named \"get_b\" not found")
            .id;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

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
        let func_lib = test_func_lib();

        let _get_b_node_id = graph
            .by_name("get_b")
            .expect("Node named \"get_b\" not found")
            .id;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

        assert_eq!(execution_graph.e_nodes.len(), 5);

        let invocation_order_before: Vec<usize> = execution_graph
            .e_nodes
            .iter()
            .map(|e_node| e_node.invocation_order)
            .collect();

        execution_graph.update(&graph, &func_lib);

        assert_eq!(execution_graph.e_nodes.len(), 5);
        assert!(
            execution_graph
                .e_nodes
                .iter()
                .all(|e_node| e_node.processing_state == ProcessingState::Processed),
            "Execution nodes should be processed after update"
        );
        let invocation_order_after: Vec<usize> = execution_graph
            .e_nodes
            .iter()
            .map(|e_node| e_node.invocation_order)
            .collect();
        assert_eq!(
            invocation_order_before, invocation_order_after,
            "Invocation order should remain stable across updates"
        );

        Ok(())
    }

    #[test]
    fn missing_input() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = test_func_lib();

        let get_b_node_id = graph
            .by_name("get_b")
            .expect("Node named \"get_b\" not found")
            .id;
        let sum_node_id = graph
            .by_name("sum")
            .expect("Node named \"sum\" not found")
            .id;
        let mult_node_id = graph
            .by_name("mult")
            .expect("Node named \"mult\" not found")
            .id;
        let print_node_id = graph
            .by_name("print")
            .expect("Node named \"print\" not found")
            .id;

        graph
            .by_name_mut("sum")
            .expect("Node named \"sum\" not found")
            .inputs[0]
            .binding = Binding::None;

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

        assert_eq!(execution_graph.e_nodes.len(), 5);
        assert!(
            execution_graph
                .by_id(get_b_node_id)
                .expect("Execution node for get_b missing")
                .should_invoke
        );
        assert!(
            !execution_graph
                .by_id(sum_node_id)
                .expect("Execution node for sum missing")
                .should_invoke
        );
        assert!(
            !execution_graph
                .by_id(mult_node_id)
                .expect("Execution node for mult missing")
                .should_invoke
        );
        assert!(
            !execution_graph
                .by_id(print_node_id)
                .expect("Execution node for print missing")
                .should_invoke
        );

        assert!(
            !execution_graph
                .by_id(get_b_node_id)
                .expect("Execution node for get_b missing")
                .has_missing_inputs
        );
        assert!(
            execution_graph
                .by_id(sum_node_id)
                .expect("Execution node for sum missing")
                .has_missing_inputs
        );
        assert!(
            execution_graph
                .by_id(mult_node_id)
                .expect("Execution node for mult missing")
                .has_missing_inputs
        );
        assert!(
            execution_graph
                .by_id(print_node_id)
                .expect("Execution node for print missing")
                .has_missing_inputs
        );

        let sum_node = execution_graph
            .by_id(sum_node_id)
            .expect("Execution node for sum missing");
        assert_eq!(
            sum_node.inputs[0].state,
            InputState::Missing,
            "Sum input 0 should be missing"
        );

        let mult_node = execution_graph
            .by_id(mult_node_id)
            .expect("Execution node for mult missing");
        assert_eq!(
            mult_node.inputs[0].state,
            InputState::Missing,
            "Mult input 0 should be missing due to sum"
        );

        let print_node = execution_graph
            .by_id(print_node_id)
            .expect("Execution node for print missing");
        assert_eq!(
            print_node.inputs[0].state,
            InputState::Missing,
            "Print input 0 should be missing due to mult"
        );

        Ok(())
    }

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = test_func_lib();

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

        for format in [FileFormat::Yaml, FileFormat::Json] {
            let serialized = execution_graph.serialize(format);
            let deserialized = ExecutionGraph::deserialize(serialized.as_str(), format)?;
            let serialized_again = deserialized.serialize(format);

            match format {
                FileFormat::Yaml => {
                    let value_a: serde_yml::Value = serde_yml::from_str(&serialized)?;
                    let value_b: serde_yml::Value = serde_yml::from_str(&serialized_again)?;
                    assert_eq!(value_a, value_b);
                }
                FileFormat::Json => {
                    let value_a: serde_json::Value = serde_json::from_str(&serialized)?;
                    let value_b: serde_json::Value = serde_json::from_str(&serialized_again)?;
                    assert_eq!(value_a, value_b);
                }
            }
        }

        Ok(())
    }

    #[test]
    fn execution_graph_updates_after_graph_change() {
        let mut graph = test_graph();
        let func_lib = test_func_lib();

        let get_a_node_id = graph
            .by_name("get_a")
            .expect("Node named \"get_a\" not found")
            .id;
        let get_b_node_id = graph
            .by_name("get_b")
            .expect("Node named \"get_b\" not found")
            .id;

        graph
            .by_name_mut("mult")
            .expect("Node named \"mult\" not found")
            .inputs = vec![
            Input {
                binding: Binding::from_output_binding(
                    graph
                        .by_name("get_a")
                        .expect("Node named \"get_a\" not found")
                        .id,
                    0,
                ),
                const_value: Some(StaticValue::Int(123)),
            },
            Input {
                binding: Binding::from_output_binding(
                    graph
                        .by_name("get_b")
                        .expect("Node named \"get_b\" not found")
                        .id,
                    0,
                ),
                const_value: Some(StaticValue::Int(12)),
            },
        ];

        let sum_node_id = graph
            .by_name("sum")
            .expect("Node named \"sum\" not found")
            .id;
        graph.remove_by_id(sum_node_id);

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);
        execution_graph.validate_with_graph(&graph);

        assert_eq!(graph.nodes.len(), 4);
        assert_eq!(execution_graph.e_nodes.len(), 4);

        let mult_node_id = graph
            .by_name("mult")
            .expect("Node named \"mult\" not found")
            .id;
        let mult_node = execution_graph
            .by_id(mult_node_id)
            .expect("Execution node for mult missing");
        let mult_input_a = mult_node.inputs[0]
            .output_address
            .expect("Mult input A missing output address");
        let mult_input_b = mult_node.inputs[1]
            .output_address
            .expect("Mult input B missing output address");
        assert_eq!(
            execution_graph.e_nodes[mult_input_a.e_node_idx].id, get_a_node_id,
            "Mult input A should be wired to get_a"
        );
        assert_eq!(
            execution_graph.e_nodes[mult_input_b.e_node_idx].id, get_b_node_id,
            "Mult input B should be wired to get_b"
        );
    }

    #[test]
    fn once_node_with_cached_outputs_skips_invocation() {
        let graph = test_graph();
        let func_lib = test_func_lib();

        let mut execution_graph = ExecutionGraph::default();
        execution_graph.update(&graph, &func_lib);

        let get_b_node_id = graph
            .by_name("get_b")
            .expect("Node named \"get_b\" not found")
            .id;
        execution_graph
            .by_id_mut(get_b_node_id)
            .expect("Execution node for get_b missing")
            .output_values = Some(vec![DynamicValue::Int(7)]);

        execution_graph.update(&graph, &func_lib);

        let get_b_node = execution_graph
            .by_id(get_b_node_id)
            .expect("Execution node for get_b missing");
        assert!(
            !get_b_node.should_invoke,
            "Once node with cached outputs should not invoke"
        );
    }
}
