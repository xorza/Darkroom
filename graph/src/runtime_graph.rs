use std::mem::take;

use anyhow::Result;
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::data::DynamicValue;
use crate::function::FuncLib;
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
pub enum InputState {
    #[default]
    Unknown,
    Unchanged,
    Changed,
    Missing,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct RuntimeNode {
    pub id: NodeId,
    pub name: String,
    pub behavior: NodeBehavior,

    pub inputs: Vec<InputState>,

    pub has_missing_inputs: bool,
    pub has_changed_inputs: bool,
    pub should_invoke: bool,
    pub invocation_order: u64,

    processing_state: ProcessingState,

    pub run_time: f64,

    #[serde(skip)]
    pub(crate) cache: InvokeCache,
    #[serde(skip)]
    pub(crate) output_values: Option<Vec<DynamicValue>>,

    //these used for validation that all bindings were visited
    pub(crate) output_binding_count: Vec<u32>,
    pub(crate) total_binding_count: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RuntimeGraph {
    pub r_nodes: Vec<RuntimeNode>,
    node_index_by_id: HashMap<NodeId, usize>,
}

impl RuntimeNode {
    pub(crate) fn decrement_current_binding_count(&mut self, output_index: u32) {
        assert!(
            (output_index as usize) < self.output_binding_count.len(),
            "Output index out of range: {}",
            output_index
        );
        assert!(self.output_binding_count.len() > output_index as usize);
        assert!(self.output_binding_count[output_index as usize] >= 1);
        assert!(self.total_binding_count >= 1);

        self.output_binding_count[output_index as usize] -= 1;
        self.total_binding_count -= 1;
    }

    fn reset_from(&mut self, node: &Node) {
        let mut prev_state = take(self);

        self.id = node.id;
        self.name = take(&mut prev_state.name);
        if self.name != node.name {
            self.name = node.name.clone();
        }
        self.behavior = node.behavior;

        self.inputs = take(&mut prev_state.inputs);
        self.inputs.resize(node.inputs.len(), InputState::Unknown);
        self.inputs.fill(InputState::Unknown);

        self.output_binding_count = take(&mut prev_state.output_binding_count);
        self.output_binding_count.fill(0);

        self.cache = take(&mut prev_state.cache);
        self.output_values = take(&mut prev_state.output_values);
    }
}

impl RuntimeGraph {
    pub fn node_by_id(&self, node_id: NodeId) -> Option<&RuntimeNode> {
        self.node_index_by_id
            .get(&node_id)
            .map(|&index| &self.r_nodes[index])
    }
    pub fn node_by_id_mut(&mut self, node_id: NodeId) -> Option<&mut RuntimeNode> {
        self.node_index_by_id
            .get(&node_id)
            .copied()
            .map(move |index| &mut self.r_nodes[index])
    }

    // mark active nodes without cached outputs for execution
    pub fn next(&mut self, graph: &Graph, func_lib: &FuncLib) {
        self.build_node_cache(graph);
        self.propagate_input_state(graph, func_lib);
    }

    fn build_node_cache(&mut self, graph: &Graph) {
        for node in graph.nodes.iter() {
            assert!(!node.id.is_nil(), "Graph node has invalid id");

            let r_node_index = match self.node_index_by_id.get(&node.id).copied() {
                Some(index) => index,
                None => {
                    self.r_nodes.push(RuntimeNode::default());
                    self.r_nodes.len() - 1
                }
            };

            self.r_nodes[r_node_index].reset_from(node);
        }

        self.rebuild_node_index();
    }

    // mark missing inputs and propagate behavior based on upstream active nodes
    fn propagate_input_state(&mut self, graph: &Graph, func_lib: &FuncLib) {
        let graph_node_index_by_id = graph.build_node_index_by_id();
        let active_node_ids = self.build_active_node_ids_ordered(graph, &graph_node_index_by_id);

        for (invocation_order, &node_id) in active_node_ids.iter().enumerate() {
            let node_index = *self
                .node_index_by_id
                .get(&node_id)
                .expect("Node index not found");

            {
                let r_node = &mut self.r_nodes[node_index];
                assert_eq!(
                    r_node.processing_state,
                    ProcessingState::Processed,
                    "Runtime node must be processed before input propagation"
                );
                // println!("{}", r_node.name);

                r_node.invocation_order = invocation_order as u64;
                // avoid traversing inputs for NodeBehavior::Once nodes having outputs
                // even if having missing inputs
                if r_node.behavior == NodeBehavior::Once && r_node.output_values.is_some() {
                    // should_invoke is false
                    continue;
                }
            }

            let node = &graph.nodes[*graph_node_index_by_id
                .get(&node_id)
                .expect("Runtime node missing from graph")];

            let mut has_changed_inputs = false;
            let mut has_missing_inputs = false;

            for (input_idx, input) in node.inputs.iter().enumerate() {
                let input_state = match &input.binding {
                    Binding::None => InputState::Missing,
                    // todo implement notifying of const binding changes
                    Binding::Const => InputState::Changed,
                    Binding::Output(output_binding) => {
                        let output_r_node = self
                            .node_by_id(output_binding.output_node_id)
                            .expect("Output binding references missing node");

                        assert_eq!(output_r_node.processing_state, ProcessingState::Processed);

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
                        let func = func_lib
                            .func_by_id(node.func_id)
                            .expect("Missing function for node during input propagation");
                        assert!(
                            input_idx < func.inputs.len(),
                            "Node input index {} out of range for function {:?}",
                            input_idx,
                            node.func_id
                        );
                        let is_required = func.inputs[input_idx].is_required;
                        has_missing_inputs |= is_required;
                    }
                    InputState::Unknown => panic!("unprocessed input"),
                }

                self.r_nodes[node_index].inputs[input_idx] = input_state;
            }

            let r_node = &mut self.r_nodes[node_index];
            r_node.has_missing_inputs = has_missing_inputs;
            if !r_node.has_missing_inputs {
                match r_node.behavior {
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

    fn build_active_node_ids_ordered(
        &mut self,
        graph: &Graph,
        graph_node_index_by_id: &HashMap<NodeId, usize>,
    ) -> Vec<NodeId> {
        let node_count = graph.nodes.len();
        let mut result: Vec<NodeId> = Vec::with_capacity(node_count);

        enum VisitCause {
            Terminal,
            OutputRequest { output_index: u32 },
            Processed,
        }
        struct Visit {
            node_id: NodeId,
            cause: VisitCause,
        }
        let mut stack: Vec<Visit> = Vec::with_capacity(10);

        graph
            .nodes
            .iter()
            .filter(|&node| node.behavior == NodeBehavior::Terminal)
            .for_each(|node| {
                stack.push(Visit {
                    node_id: node.id,
                    cause: VisitCause::Terminal,
                });
            });

        while let Some(visit) = stack.pop() {
            let r_node_index = self
                .node_index_by_id
                .get(&visit.node_id)
                .copied()
                .expect("node not found");
            let r_node = &mut self.r_nodes[r_node_index];
            // println!("{}", r_node.name);

            match visit.cause {
                VisitCause::Terminal => {}
                VisitCause::OutputRequest { output_index } => {
                    let output_index = output_index as usize;
                    r_node.output_binding_count.resize(output_index + 1, 0);
                    r_node.output_binding_count[output_index] += 1;
                    r_node.total_binding_count += 1;
                }
                VisitCause::Processed => {
                    r_node.processing_state = ProcessingState::Processed;
                    result.push(r_node.id);
                    continue;
                }
            }

            match r_node.processing_state {
                ProcessingState::Processed => {
                    continue;
                }
                ProcessingState::Processing => {
                    // todo replace with result<>
                    panic!("Cycle detected while building runtime graph");
                }
                ProcessingState::None => {}
            }

            r_node.processing_state = ProcessingState::Processing;
            stack.push(Visit {
                node_id: visit.node_id,
                cause: VisitCause::Processed,
            });

            let node = &graph.nodes[*graph_node_index_by_id
                .get(&visit.node_id)
                .expect("Node missing from graph")];
            for input in node.inputs.iter() {
                if let Binding::Output(output_binding) = &input.binding {
                    stack.push(Visit {
                        node_id: output_binding.output_node_id,
                        cause: VisitCause::OutputRequest {
                            output_index: output_binding.output_index,
                        },
                    });
                }
            }
        }

        result
    }

    fn rebuild_node_index(&mut self) {
        self.node_index_by_id.clear();
        self.node_index_by_id.reserve(self.r_nodes.len());

        for (index, node) in self.r_nodes.iter().enumerate() {
            let prev = self.node_index_by_id.insert(node.id, index);

            assert!(
                prev.is_none(),
                "Duplicate runtime node id detected: {:?}",
                node.id
            );
        }
    }
}

fn validate_runtime_inputs(graph: &Graph, func_lib: &FuncLib) -> Result<()> {
    graph.validate()?;

    for node in graph.nodes.iter() {
        let func = func_lib.func_by_id(node.func_id).ok_or_else(|| {
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
                    .node_by_id(output_binding.output_node_id)
                    .ok_or_else(|| anyhow::Error::msg("Output binding references missing node"))?;
                let output_func = func_lib.func_by_id(output_node.func_id).ok_or_else(|| {
                    anyhow::Error::msg(format!(
                        "Missing function {:?} for output node {:?}",
                        output_node.func_id, output_node.id
                    ))
                })?;
                if (output_binding.output_index as usize) >= output_func.outputs.len() {
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

    #[test]
    fn simple_run() -> anyhow::Result<()> {
        let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let get_b_node_id = graph
            .node_by_name("get_b")
            .unwrap_or_else(|| panic!("Node named \"get_b\" not found"))
            .id;

        let mut runtime_graph = RuntimeGraph::default();
        runtime_graph.next(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .total_binding_count,
            2
        );
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
        let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let get_b_node_id = graph
            .node_by_name("get_b")
            .unwrap_or_else(|| panic!("Node named \"get_b\" not found"))
            .id;

        let mut runtime_graph = RuntimeGraph::default();
        runtime_graph.next(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .total_binding_count,
            2
        );

        runtime_graph.next(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .total_binding_count,
            2
        );

        Ok(())
    }

    #[test]
    fn missing_input() -> anyhow::Result<()> {
        let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let get_b_node_id = graph
            .node_by_name("get_b")
            .unwrap_or_else(|| panic!("Node named \"get_b\" not found"))
            .id;
        let sum_node_id = graph
            .node_by_name("sum")
            .unwrap_or_else(|| panic!("Node named \"sum\" not found"))
            .id;
        let mult_node_id = graph
            .node_by_name("mult")
            .unwrap_or_else(|| panic!("Node named \"mult\" not found"))
            .id;
        let print_node_id = graph
            .node_by_name("print")
            .unwrap_or_else(|| panic!("Node named \"print\" not found"))
            .id;

        graph
            .node_by_name_mut("sum")
            .unwrap_or_else(|| panic!("Node named \"sum\" not found"))
            .inputs[0]
            .binding = Binding::None;

        let mut runtime_graph = RuntimeGraph::default();
        runtime_graph.next(&graph, &func_lib);

        assert_eq!(runtime_graph.r_nodes.len(), 5);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .total_binding_count,
            2
        );

        assert!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .should_invoke
        );
        assert!(
            !runtime_graph
                .node_by_id(sum_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", sum_node_id))
                .should_invoke
        );
        assert!(
            !runtime_graph
                .node_by_id(mult_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", mult_node_id))
                .should_invoke
        );
        assert!(
            !runtime_graph
                .node_by_id(print_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", print_node_id))
                .should_invoke
        );

        assert!(
            !runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .node_by_id(sum_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", sum_node_id))
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .node_by_id(mult_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", mult_node_id))
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .node_by_id(print_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", print_node_id))
                .has_missing_inputs
        );

        let _yaml = serde_yml::to_string(&runtime_graph)?;

        Ok(())
    }
}
