use std::collections::VecDeque;
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

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct RuntimeNode {
    pub id: NodeId,
    pub name: String,
    pub behavior: NodeBehavior,

    // required inputs
    pub inputs: Vec<bool>,

    pub has_missing_inputs: bool,
    pub should_invoke: bool,

    processing_state: ProcessingState,

    pub run_time: f64,

    #[serde(skip)]
    pub(crate) cache: InvokeCache,
    #[serde(skip)]
    pub(crate) output_values: Option<Vec<DynamicValue>>,

    //these used for validation that all binding were visited
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
        assert!(self.output_binding_count[output_index as usize] >= 1);
        assert!(self.total_binding_count >= 1);

        self.output_binding_count[output_index as usize] -= 1;
        self.total_binding_count -= 1;
    }

    fn reset_from(&mut self, node: &Node, func: &Func) {
        self.name = node.name.clone();
        self.behavior = node.behavior;

        // todo optimize to reuse memory
        self.inputs = func.inputs.iter().map(|i| i.is_required).collect();

        self.has_missing_inputs = false;
        self.should_invoke = false;
        self.total_binding_count = 0;

        self.run_time = 0.0;

        self.output_binding_count.resize(func.outputs.len(), 0);
        self.output_binding_count.fill(0);
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
        self.build_node_cache(graph, func_lib);
        self.rebuild_node_index();
        self.reset_runtime_state();
        self.propagate_missing_inputs_and_behavior(graph);
    }

    fn build_node_cache(&mut self, graph: &Graph, func_lib: &FuncLib) {
        for node in graph.nodes.iter() {
            assert!(!node.id.is_nil(), "Graph node has invalid id");

            let func = func_lib.func_by_id(node.func_id).unwrap_or_else(|| {
                panic!(
                    "Missing function {:?} for node {:?}",
                    node.func_id, node.name
                )
            });

            let r_node_index = match self.node_index_by_id.get(&node.id).copied() {
                Some(index) => index,
                None => {
                    self.r_nodes.push(RuntimeNode {
                        id: node.id,
                        ..Default::default()
                    });

                    self.r_nodes.len() - 1
                }
            };

            let r_node = &mut self.r_nodes[r_node_index];
            assert_eq!(r_node.id, node.id, "");
            r_node.reset_from(node, func);
        }
    }

    // mark missing inputs and propagate behavior based on upstream active nodes
    fn propagate_missing_inputs_and_behavior(&mut self, graph: &Graph) {
        let graph_node_index_by_id = graph.build_node_index_by_id();
        let active_node_indices =
            self.collect_ordered_active_node_ids(graph, &graph_node_index_by_id);

        for index in active_node_indices {
            let mut r_node = take(&mut self.r_nodes[index]);
            println!("{}", r_node.name);

            let node = &graph.nodes[*graph_node_index_by_id
                .get(&r_node.id)
                .expect("Runtime node missing from graph")];

            for (input_idx, input) in node.inputs.iter().enumerate() {
                match &input.binding {
                    Binding::None => {
                        r_node.has_missing_inputs |= r_node.inputs[input_idx];
                    }
                    Binding::Const => {}
                    Binding::Output(output_binding) => {
                        let output_r_node = self
                            .r_nodes
                            .iter()
                            .find(|&p_node| p_node.id == output_binding.output_node_id)
                            .expect("node not found");

                        if output_r_node.has_missing_inputs {
                            r_node.has_missing_inputs = true;
                        } else {
                            if output_r_node.should_invoke {
                                match r_node.behavior {
                                    NodeBehavior::Terminal
                                    | NodeBehavior::OnInputChange
                                    | NodeBehavior::Always => {
                                        r_node.should_invoke = true;
                                    }
                                    NodeBehavior::Once => {
                                        r_node.should_invoke = r_node.output_values.is_none();
                                    }
                                }
                            }
                        }
                    }
                }
            }

            self.r_nodes[index] = r_node;
        }
    }

    fn collect_ordered_active_node_ids(
        &mut self,
        graph: &Graph,
        graph_node_index_by_id: &HashMap<NodeId, usize>,
    ) -> Vec<usize> {
        let node_count = graph.nodes.len();
        let mut result: Vec<usize> = Vec::with_capacity(node_count);

        enum VisitCause {
            Terminal,
            OutputRequest { output_index: u32 },
            Processed,
        }
        struct Visit {
            node_id: NodeId,
            cause: VisitCause,
        }
        let mut stack: VecDeque<Visit> = VecDeque::with_capacity(10);

        graph
            .nodes
            .iter()
            .filter(|&node| node.behavior == NodeBehavior::Terminal)
            .for_each(|node| {
                stack.push_back(Visit {
                    node_id: node.id,
                    cause: VisitCause::Terminal,
                });
            });

        while let Some(visit) = stack.pop_back() {
            let r_node_index = self
                .node_index_by_id
                .get(&visit.node_id)
                .copied()
                .expect("node not found");
            let r_node = &mut self.r_nodes[r_node_index];

            match visit.cause {
                VisitCause::Terminal => {}
                VisitCause::OutputRequest { output_index } => {
                    r_node.output_binding_count[output_index as usize] += 1;
                    r_node.total_binding_count += 1;
                }
                VisitCause::Processed => {
                    r_node.processing_state = ProcessingState::Processed;
                    result.push(r_node_index);
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
            stack.push_back(Visit {
                node_id: visit.node_id,
                cause: VisitCause::Processed,
            });

            let node = &graph.nodes[*graph_node_index_by_id
                .get(&visit.node_id)
                .expect("Node missing from graph")];
            for input in node.inputs.iter() {
                if let Binding::Output(output_binding) = &input.binding {
                    stack.push_back(Visit {
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

    fn reset_runtime_state(&mut self) {
        self.r_nodes.iter_mut().for_each(|r_node| {
            r_node.should_invoke = false;
            r_node.output_binding_count.fill(0);
            r_node.total_binding_count = 0;
        });
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
