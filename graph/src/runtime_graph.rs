use std::mem::take;

use anyhow::Result;
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::data::DynamicValue;
use crate::function::FuncBehavior;
use crate::function::{Func, FuncLib};
use crate::graph::{Binding, Graph, Node, NodeId};
use crate::invoke::InvokeCache;

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct RuntimeNode {
    pub id: NodeId,

    // terminal means the node has side effects (file writes, ...)
    pub terminal: bool,
    pub behavior: FuncBehavior,
    pub cache_outputs: bool,

    pub has_missing_inputs: bool,
    pub should_invoke: bool,
    pub run_time: f64,

    #[serde(skip)]
    pub(crate) cache: InvokeCache,
    #[serde(skip)]
    pub(crate) output_values: Option<Vec<DynamicValue>>,

    pub(crate) output_binding_count: Vec<u32>,
    pub(crate) total_binding_count: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RuntimeGraph {
    pub r_nodes: Vec<RuntimeNode>,
    node_index_by_id: HashMap<NodeId, usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Visited,
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
        self.terminal = node.terminal;
        self.cache_outputs = node.cache_outputs;
        self.behavior = func.behavior;

        self.has_missing_inputs = false;
        self.run_time = 0.0;
        self.should_invoke = false;
        self.total_binding_count = 0;

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
    pub fn next(&mut self, graph: &Graph) {
        Self::reset_runtime_state(&mut self.r_nodes);

        let mut active_node_ids = Self::terminal_node_ids(&self.r_nodes);

        let mut index = 0;
        while index < active_node_ids.len() {
            index += 1;
            let index = index - 1;

            let node_id = active_node_ids[index];
            let node = graph
                .node_by_id(node_id)
                .unwrap_or_else(|| panic!("Node with id {:?} not found", node_id));
            let r_node = self
                .r_nodes
                .get_mut(
                    *self
                        .node_index_by_id
                        .get(&node_id)
                        .expect("Runtime node missing"),
                )
                .expect("Runtime node missing");

            let is_active = Self::should_execute_or_propagate(r_node);
            r_node.should_invoke = is_active && !r_node.has_missing_inputs;

            node.inputs.iter().for_each(|input| {
                if let Binding::Output(output_binding) = &input.binding {
                    if is_active {
                        active_node_ids.push(output_binding.output_node_id);
                    }
                    let output_r_node = self
                        .r_nodes
                        .get_mut(
                            *self
                                .node_index_by_id
                                .get(&output_binding.output_node_id)
                                .expect("Runtime node missing"),
                        )
                        .expect("Runtime node missing");
                    output_r_node.output_binding_count[output_binding.output_index as usize] += 1;
                    output_r_node.total_binding_count += 1;
                }
            });
        }
    }

    // update graph
    pub fn update(&mut self, graph: &Graph, func_lib: &FuncLib) {
        validate_runtime_inputs(graph, func_lib)
            .expect("RuntimeGraph build requires a validated graph and function library");

        self.r_nodes.reserve(graph.nodes.len());

        let graph_node_index_by_id = graph.build_node_index_by_id();
        let active_node_ids = collect_ordered_terminal_dependencies(graph, &graph_node_index_by_id);

        for node_id in active_node_ids {
            let node_index = *graph_node_index_by_id
                .get(&node_id)
                .expect("Missing graph node for runtime node");
            let node = &graph.nodes[node_index];
            let func = func_lib
                .func_by_id(node.func_id)
                .unwrap_or_else(|| panic!("Func with id {:?} not found", node.func_id));

            let r_node = if let Some(&index) = self.node_index_by_id.get(&node_id) {
                &mut self.r_nodes[index]
            } else {
                self.r_nodes.push(RuntimeNode {
                    id: node_id,
                    ..Default::default()
                });
                self.r_nodes.last_mut().unwrap()
            };
            r_node.reset_from(node, func);
        }

        self.rebuild_node_index();

        Self::propagate_missing_inputs_and_behavior(
            graph,
            func_lib,
            &graph_node_index_by_id,
            &mut self.r_nodes,
        );
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

    // mark missing inputs and propagate behavior based on upstream active nodes
    fn propagate_missing_inputs_and_behavior(
        graph: &Graph,
        func_lib: &FuncLib,
        graph_node_index_by_id: &HashMap<NodeId, usize>,
        r_nodes: &mut [RuntimeNode],
    ) {
        for index in 0..r_nodes.len() {
            let mut r_node = take(&mut r_nodes[index]);
            let node = &graph.nodes[*graph_node_index_by_id
                .get(&r_node.id)
                .expect("Runtime node missing from graph")];
            let func = func_lib
                .func_by_id(node.func_id)
                .unwrap_or_else(|| panic!("Func with id {:?} not found", node.func_id));

            node.inputs
                .iter()
                .enumerate()
                .for_each(|(idx, input)| match &input.binding {
                    Binding::None => {
                        r_node.has_missing_inputs |= func.inputs[idx].is_required;
                    }
                    Binding::Const => {}
                    Binding::Output(output_binding) => {
                        let output_r_node = r_nodes[0..index]
                            .iter()
                            .find(|&p_node| p_node.id == output_binding.output_node_id)
                            .expect("Node not found among already processed ones");
                        if output_r_node.behavior == FuncBehavior::Impure {
                            r_node.behavior = FuncBehavior::Impure;
                        }
                        r_node.has_missing_inputs |= output_r_node.has_missing_inputs;
                    }
                });

            if r_node.behavior == FuncBehavior::Pure {
                r_node.cache_outputs = true;
            }
            r_nodes[index] = r_node;
        }
    }

    fn reset_runtime_state(r_nodes: &mut [RuntimeNode]) {
        r_nodes.iter_mut().for_each(|r_node| {
            r_node.should_invoke = false;
            r_node.output_binding_count.fill(0);
            r_node.total_binding_count = 0;
        });
    }

    fn terminal_node_ids(r_nodes: &[RuntimeNode]) -> Vec<NodeId> {
        r_nodes
            .iter()
            .filter_map(|r_node| {
                if r_node.terminal {
                    Some(r_node.id)
                } else {
                    None
                }
            })
            .collect()
    }

    fn should_execute_or_propagate(r_node: &RuntimeNode) -> bool {
        r_node.terminal || r_node.output_values.is_none() || r_node.behavior == FuncBehavior::Impure
    }
}

fn collect_ordered_terminal_dependencies(
    graph: &Graph,
    graph_node_index_by_id: &HashMap<NodeId, usize>,
) -> Vec<NodeId> {
    let node_count = graph.nodes.len();
    let mut visit_state: HashMap<NodeId, VisitState> = HashMap::with_capacity(node_count);
    let mut ordered: Vec<NodeId> = Vec::with_capacity(node_count);
    let mut stack: Vec<(NodeId, bool)> = Vec::with_capacity(10);

    for node in graph.nodes.iter().filter(|node| node.terminal) {
        stack.push((node.id, false));
    }

    while let Some((node_id, expanded)) = stack.pop() {
        if expanded {
            visit_state.insert(node_id, VisitState::Visited);
            ordered.push(node_id);
            continue;
        }

        match visit_state.get(&node_id) {
            Some(VisitState::Visited) => continue,
            Some(VisitState::Visiting) => {
                panic!("Cycle detected while building runtime graph");
            }
            None => {}
        }

        visit_state.insert(node_id, VisitState::Visiting);
        stack.push((node_id, true));

        let node_index = *graph_node_index_by_id
            .get(&node_id)
            .expect("Node missing from graph");
        let node = &graph.nodes[node_index];
        for input in node.inputs.iter() {
            if let Binding::Output(output_binding) = &input.binding {
                stack.push((output_binding.output_node_id, false));
            }
        }
    }

    ordered
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
    use crate::function::FuncLib;
    use crate::graph::{Binding, Graph};
    use crate::runtime_graph::RuntimeGraph;

    #[test]
    fn simple_run() -> anyhow::Result<()> {
        let graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let get_b_node_id = graph
            .node_by_name("get_b")
            .unwrap_or_else(|| panic!("Node named \"get_b\" not found"))
            .id;

        let mut runtime_graph = RuntimeGraph::default();
        runtime_graph.update(&graph, &func_lib);
        runtime_graph.next(&graph);

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
        runtime_graph.update(&graph, &func_lib);
        runtime_graph.next(&graph);

        assert_eq!(runtime_graph.r_nodes.len(), 5);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .total_binding_count,
            2
        );

        runtime_graph.next(&graph);

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
        runtime_graph.update(&graph, &func_lib);
        runtime_graph.next(&graph);

        assert_eq!(runtime_graph.r_nodes.len(), 4);
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
