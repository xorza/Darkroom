use std::mem::take;

use anyhow::Result;
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::data::DynamicValue;
use crate::function::{Func, FuncLib};
use crate::graph::{Binding, Graph, Node, NodeBehavior, NodeId};
use crate::invoke::InvokeCache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
enum ProcessingState {
    #[default]
    None,
    Processing,
    Processed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RuntimePortAddress {
    pub r_node_idx: usize,
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
pub struct RuntimeInput {
    pub state: InputState,
    pub required: bool,
    pub output_address: Option<RuntimePortAddress>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RuntimeOutput {
    #[default]
    Unused,
    Used,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct RuntimeNode {
    pub id: NodeId,

    pub has_missing_inputs: bool,
    pub has_changed_inputs: bool,
    pub should_invoke: bool,
    pub invocation_order: usize,

    pub node_idx: usize,
    pub func_idx: usize,
    pub inputs: Vec<RuntimeInput>,
    pub outputs: Vec<RuntimeOutput>,

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
pub struct RuntimeGraph {
    pub r_nodes: Vec<RuntimeNode>,
    r_node_idx_by_id: HashMap<NodeId, usize>,
}

impl RuntimeNode {
    fn reset_from(&mut self, node: &Node) {
        let mut prev_state = take(self);

        self.id = node.id;

        self.inputs = take(&mut prev_state.inputs);
        self.inputs.fill(RuntimeInput::default());
        self.inputs
            .resize(node.inputs.len(), RuntimeInput::default());

        self.outputs = take(&mut prev_state.outputs);
        self.outputs.fill(RuntimeOutput::default());

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
            self.inputs.push(RuntimeInput {
                required: input.required,
                ..Default::default()
            });
        }

        self.outputs.fill(RuntimeOutput::Unused);
        self.outputs
            .resize(func.outputs.len(), RuntimeOutput::default());
    }
}

impl RuntimeGraph {
    pub fn by_id(&self, node_id: NodeId) -> Option<&RuntimeNode> {
        self.r_nodes.iter().find(|node| node.id == node_id)
    }
    pub fn by_id_mut(&mut self, node_id: NodeId) -> Option<&mut RuntimeNode> {
        self.r_nodes.iter_mut().find(|node| node.id == node_id)
    }

    // Rebuild runtime state from the current graph and function library.
    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) {
        self.update_node_cache(graph);
        self.backward(graph, func_lib);
        self.forward(graph);
    }

    // Update the node cache with the current graph.
    fn update_node_cache(&mut self, graph: &Graph) {
        // Compact r_nodes in-place to keep only nodes that still exist in graph.
        // We reuse existing RuntimeNode slots to avoid extra allocations.
        let mut write_idx = 0;
        for (node_idx, node) in graph.nodes.iter().enumerate() {
            assert!(
                !node.id.is_nil(),
                "Graph node has nil id at index {}",
                node_idx
            );

            // Look up the current slot for this node id (if any), otherwise append a new slot.
            let r_node_idx = match self.r_node_idx_by_id.get(&node.id).copied() {
                Some(idx) => idx,
                None => {
                    if write_idx >= self.r_nodes.len() {
                        self.r_nodes.push(RuntimeNode::default());
                    }
                    write_idx
                }
            };

            // Move the runtime node we want into the next compacted slot.
            if r_node_idx != write_idx {
                self.r_nodes.swap(r_node_idx, write_idx);
                let swapped_id = self.r_nodes[r_node_idx].id;
                if !swapped_id.is_nil() {
                    // The swapped node moved; update its cached index.
                    self.r_node_idx_by_id.insert(swapped_id, r_node_idx);
                }
            }

            // Reset the runtime node with the latest graph node data.
            let r_node = &mut self.r_nodes[write_idx];
            r_node.reset_from(node);
            r_node.node_idx = node_idx;
            self.r_node_idx_by_id.insert(node.id, write_idx);
            write_idx += 1;
        }

        // Drop any runtime nodes past the compacted range.
        self.r_nodes.truncate(write_idx);
        // Prune stale id->index entries that point past the new length or mismatched ids.
        self.r_node_idx_by_id
            .retain(|id, idx| *idx < self.r_nodes.len() && self.r_nodes[*idx].id == *id);

        #[cfg(debug_assertions)]
        {
            assert_eq!(
                self.r_nodes.len(),
                self.r_node_idx_by_id.len(),
                "Runtime node count mismatch"
            );
            assert_eq!(
                self.r_nodes.len(),
                graph.nodes.len(),
                "Runtime node count mismatch"
            );
            // Check that the runtime graph is in a consistent state.
            self.r_nodes.iter().enumerate().for_each(|(idx, r_node)| {
                assert_eq!(
                    idx, self.r_node_idx_by_id[&r_node.id],
                    "Runtime node index mismatch"
                );
                assert!(
                    r_node.node_idx < graph.nodes.len(),
                    "Runtime node index out of bounds"
                );
                assert_eq!(
                    graph.nodes[r_node.node_idx].id, r_node.id,
                    "Runtime node id mismatch"
                );
            });
        }
    }

    // Walk upstream dependencies to mark active nodes and compute invocation order.
    fn backward(&mut self, graph: &Graph, func_lib: &FuncLib) {
        enum VisitCause {
            Terminal,
            OutputRequest { output_address: RuntimePortAddress },
            Processed,
        }
        struct Visit {
            r_node_idx: usize,
            cause: VisitCause,
        }
        let mut stack: Vec<Visit> = Vec::with_capacity(10);

        self.r_nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, r_node)| {
                if graph.nodes[r_node.node_idx].behavior == NodeBehavior::Terminal {
                    Some(idx)
                } else {
                    None
                }
            })
            .for_each(|idx| {
                stack.push(Visit {
                    r_node_idx: idx,
                    cause: VisitCause::Terminal,
                });
            });

        let mut invocation_order: usize = 0;
        while let Some(visit) = stack.pop() {
            let r_node_idx = visit.r_node_idx;
            let node_idx = {
                let r_node = &mut self.r_nodes[r_node_idx];
                // println!("{}", r_node.name);

                let output_address = match visit.cause {
                    VisitCause::Terminal => None,
                    VisitCause::OutputRequest { output_address } => {
                        assert_eq!(
                            visit.r_node_idx, output_address.r_node_idx,
                            "Visit r_node_idx {} does not match output address r_node_idx {}",
                            visit.r_node_idx, output_address.r_node_idx
                        );
                        Some(output_address)
                    }
                    VisitCause::Processed => {
                        r_node.processing_state = ProcessingState::Processed;
                        r_node.invocation_order = invocation_order;
                        invocation_order += 1;
                        continue;
                    }
                };

                match r_node.processing_state {
                    ProcessingState::Processed => {
                        continue;
                    }
                    ProcessingState::Processing => {
                        // todo replace with result<>
                        panic!(
                            "Cycle detected while building runtime graph at node {:?}",
                            visit.r_node_idx
                        );
                    }
                    ProcessingState::None => {
                        let func_idx = func_lib
                            .funcs
                            .iter()
                            .position(|func| func.id == graph.nodes[r_node.node_idx].func_id)
                            .expect("FuncLib missing function for graph node func_id");
                        r_node.reset_ports_from_func(&func_lib.funcs[func_idx]);
                        r_node.func_idx = func_idx;
                    }
                }

                if let Some(output_address) = output_address {
                    r_node.outputs[output_address.port_idx] = RuntimeOutput::Used;
                }
                r_node.processing_state = ProcessingState::Processing;
                stack.push(Visit {
                    r_node_idx,
                    cause: VisitCause::Processed,
                });

                r_node.node_idx
            };

            for (input_idx, input) in graph.nodes[node_idx].inputs.iter().enumerate() {
                if let Binding::Output(output_binding) = &input.binding {
                    let output_r_node_idx = self.r_node_idx_by_id[&output_binding.output_node_id];
                    self.r_nodes[r_node_idx].inputs[input_idx].output_address =
                        Some(RuntimePortAddress {
                            r_node_idx: output_r_node_idx,
                            port_idx: output_binding.output_idx,
                        });
                    stack.push(Visit {
                        r_node_idx: output_r_node_idx,
                        cause: VisitCause::OutputRequest {
                            output_address: RuntimePortAddress {
                                r_node_idx: output_r_node_idx,
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
        let mut active_r_node_indices = self
            .r_nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, r_node)| {
                (r_node.processing_state == ProcessingState::Processed).then_some(idx)
            })
            .collect::<Vec<_>>();
        active_r_node_indices.sort_unstable_by_key(|&idx| self.r_nodes[idx].invocation_order);

        for r_node_idx in active_r_node_indices {
            let node = {
                let r_node = &mut self.r_nodes[r_node_idx];
                assert_eq!(
                    r_node.processing_state,
                    ProcessingState::Processed,
                    "Runtime node must be processed before input propagation"
                );
                // println!("{}", r_node.name);

                let node = &graph.nodes[r_node.node_idx];
                // avoid traversing inputs for NodeBehavior::Once nodes having outputs
                // even if having missing inputs
                if node.behavior == NodeBehavior::Once && r_node.output_values.is_some() {
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
                        let output_r_node_idx = self.r_nodes[r_node_idx].inputs[input_idx]
                            .output_address
                            .expect("Output binding references missing runtime node")
                            .r_node_idx;
                        let output_r_node = &self.r_nodes[output_r_node_idx];

                        assert_eq!(
                            output_r_node.processing_state,
                            ProcessingState::Processed,
                            "Output runtime node {:?} not processed before input propagation",
                            output_binding.output_node_id
                        );

                        if output_r_node.has_missing_inputs {
                            InputState::Missing
                        } else if output_r_node.should_invoke {
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
                        has_missing_inputs |= self.r_nodes[r_node_idx].inputs[input_idx].required;
                    }
                    InputState::Unknown => panic!("unprocessed input"),
                }

                self.r_nodes[r_node_idx].inputs[input_idx].state = input_state;
            }

            let r_node = &mut self.r_nodes[r_node_idx];
            r_node.has_missing_inputs = has_missing_inputs;
            if !r_node.has_missing_inputs {
                match node.behavior {
                    NodeBehavior::Terminal | NodeBehavior::Always => {
                        r_node.should_invoke = true;
                    }
                    NodeBehavior::Once => {
                        // has no cached outputs, so should_invoke = true
                        r_node.should_invoke = true;
                    }
                    NodeBehavior::OnInputChange => {
                        r_node.should_invoke = has_changed_inputs;
                    }
                }
            }
        }
    }
}

fn validate_runtime_inputs(graph: &Graph, func_lib: &FuncLib) -> Result<()> {
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
    use crate::graph::test_graph;

    #[test]
    fn simple_run() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let _get_b_node_id = graph
            .by_name("get_b")
            .expect("Node named \"get_b\" not found")
            .id;

        let mut runtime_graph = RuntimeGraph::default();
        runtime_graph.update(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);
        assert!(runtime_graph
            .r_nodes
            .iter()
            .all(|r_node| r_node.should_invoke));
        assert!(runtime_graph
            .r_nodes
            .iter()
            .all(|r_node| !r_node.has_missing_inputs));

        let _yaml = serde_yml::to_string(&runtime_graph)?;

        Ok(())
    }

    #[test]
    fn empty_run() -> anyhow::Result<()> {
        let graph = test_graph();
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let _get_b_node_id = graph
            .by_name("get_b")
            .expect("Node named \"get_b\" not found")
            .id;

        let mut runtime_graph = RuntimeGraph::default();
        runtime_graph.update(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);

        runtime_graph.update(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);

        Ok(())
    }

    #[test]
    fn missing_input() -> anyhow::Result<()> {
        let mut graph = test_graph();
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

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

        let mut runtime_graph = RuntimeGraph::default();
        runtime_graph.update(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);
        assert!(
            runtime_graph
                .by_id(get_b_node_id)
                .expect("Runtime node for get_b missing")
                .should_invoke
        );
        assert!(
            !runtime_graph
                .by_id(sum_node_id)
                .expect("Runtime node for sum missing")
                .should_invoke
        );
        assert!(
            !runtime_graph
                .by_id(mult_node_id)
                .expect("Runtime node for mult missing")
                .should_invoke
        );
        assert!(
            !runtime_graph
                .by_id(print_node_id)
                .expect("Runtime node for print missing")
                .should_invoke
        );

        assert!(
            !runtime_graph
                .by_id(get_b_node_id)
                .expect("Runtime node for get_b missing")
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .by_id(sum_node_id)
                .expect("Runtime node for sum missing")
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .by_id(mult_node_id)
                .expect("Runtime node for mult missing")
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .by_id(print_node_id)
                .expect("Runtime node for print missing")
                .has_missing_inputs
        );

        let _yaml = serde_yml::to_string(&runtime_graph)?;

        Ok(())
    }
}
