use std::mem::take;

use anyhow::Result;
use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::data::DynamicValue;
use crate::function::FuncBehavior;
use crate::function::FuncLib;
use crate::graph::{Binding, Graph, NodeId};
use crate::invoke::InvokeCache;

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct RuntimeNode {
    id: NodeId,

    pub terminal: bool,
    pub has_missing_inputs: bool,
    pub behavior: FuncBehavior,
    pub cache_outputs: bool,
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
    pub nodes: Vec<RuntimeNode>,
    node_index_by_id: HashMap<NodeId, usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Visited,
}

impl RuntimeNode {
    pub fn id(&self) -> NodeId {
        self.id
    }

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
}

impl RuntimeGraph {
    pub fn new(graph: &Graph, func_lib: &FuncLib) -> Self {
        Self::build_runtime_graph(graph, func_lib, &mut RuntimeGraph::default())
    }

    pub fn node_by_id(&self, node_id: NodeId) -> Option<&RuntimeNode> {
        self.node_index_by_id
            .get(&node_id)
            .map(|&index| &self.nodes[index])
    }
    pub fn node_by_id_mut(&mut self, node_id: NodeId) -> Option<&mut RuntimeNode> {
        self.node_index_by_id
            .get(&node_id)
            .copied()
            .map(move |index| &mut self.nodes[index])
    }

    pub fn next(&mut self, graph: &Graph) {
        Self::schedule_invocations_with_index(graph, &self.node_index_by_id, &mut self.nodes);
    }
}

impl RuntimeGraph {
    fn build_runtime_graph(
        graph: &Graph,
        func_lib: &FuncLib,
        previous_runtime: &mut RuntimeGraph,
    ) -> RuntimeGraph {
        Self::validate_runtime_inputs(graph, func_lib)
            .expect("RuntimeGraph build requires a validated graph and function library");

        let graph_node_index_by_id = graph.node_index_by_id();
        let mut r_nodes =
            Self::build_runtime_nodes(graph, func_lib, &graph_node_index_by_id, previous_runtime);
        Self::propagate_missing_inputs_and_behavior(
            graph,
            func_lib,
            &graph_node_index_by_id,
            &mut r_nodes,
        );
        let node_index_by_id = Self::build_node_index(&r_nodes);

        RuntimeGraph {
            nodes: r_nodes,
            node_index_by_id,
        }
    }

    fn build_runtime_nodes(
        graph: &Graph,
        func_lib: &FuncLib,
        graph_node_index_by_id: &HashMap<NodeId, usize>,
        previous_runtime: &mut RuntimeGraph,
    ) -> Vec<RuntimeNode> {
        let active_node_ids =
            Self::collect_ordered_output_dependencies(graph, graph_node_index_by_id);
        let mut result = Vec::with_capacity(active_node_ids.len());

        for node_id in active_node_ids {
            let node = &graph.nodes[*graph_node_index_by_id
                .get(&node_id)
                .expect("Missing graph node for runtime node")];
            let func = func_lib
                .func_by_id(node.func_id)
                .unwrap_or_else(|| panic!("Func with id {:?} not found", node.func_id));

            let prev_r_node = previous_runtime.node_by_id_mut(node_id);

            let (cache, output_values) = if let Some(prev_r_node) = prev_r_node {
                assert_eq!(prev_r_node.output_binding_count.len(), func.outputs.len());

                (
                    take(&mut prev_r_node.cache),
                    prev_r_node.output_values.take(),
                )
            } else {
                (InvokeCache::default(), None)
            };

            result.push(RuntimeNode {
                id: node_id,
                terminal: node.terminal,
                has_missing_inputs: false,
                behavior: func.behavior,
                cache_outputs: node.cache_outputs,
                run_time: 0.0,
                should_invoke: false,
                cache,
                output_values,

                output_binding_count: vec![0; func.outputs.len()],
                total_binding_count: 0,
            });
        }

        result
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
            let node_info = func_lib
                .func_by_id(node.func_id)
                .unwrap_or_else(|| panic!("Func with id {:?} not found", node.func_id));

            node.inputs
                .iter()
                .enumerate()
                .for_each(|(idx, input)| match &input.binding {
                    Binding::None => {
                        r_node.has_missing_inputs |= node_info.inputs[idx].is_required;
                    }
                    Binding::Const => {}
                    Binding::Output(output_binding) => {
                        let output_r_node = r_nodes[0..index]
                            .iter()
                            .find(|&p_node| p_node.id == output_binding.output_node_id)
                            .expect("Node not found among already processed ones");
                        if output_r_node.behavior == FuncBehavior::Active {
                            r_node.behavior = FuncBehavior::Active;
                        }
                        r_node.has_missing_inputs |= output_r_node.has_missing_inputs;
                    }
                });

            if r_node.behavior == FuncBehavior::Passive {
                r_node.cache_outputs = true;
            }
            r_nodes[index] = r_node;
        }
    }

    // mark active nodes without cached outputs for execution
    fn schedule_invocations(graph: &Graph, r_nodes: &mut [RuntimeNode]) {
        let runtime_node_index_by_id = Self::build_node_index(r_nodes);
        Self::schedule_invocations_with_index(graph, &runtime_node_index_by_id, r_nodes);
    }

    fn schedule_invocations_with_index(
        graph: &Graph,
        runtime_node_index_by_id: &HashMap<NodeId, usize>,
        r_nodes: &mut [RuntimeNode],
    ) {
        Self::reset_runtime_state(r_nodes);

        let mut active_node_ids = Self::output_node_ids(r_nodes);

        let mut index = 0;
        while index < active_node_ids.len() {
            index += 1;
            let index = index - 1;

            let node_id = active_node_ids[index];
            let node = graph
                .node_by_id(node_id)
                .unwrap_or_else(|| panic!("Node with id {:?} not found", node_id));
            let r_node = r_nodes
                .get_mut(
                    *runtime_node_index_by_id
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
                    let output_r_node = r_nodes
                        .get_mut(
                            *runtime_node_index_by_id
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

    fn reset_runtime_state(r_nodes: &mut [RuntimeNode]) {
        r_nodes.iter_mut().for_each(|r_node| {
            r_node.should_invoke = false;
            r_node.output_binding_count.fill(0);
            r_node.total_binding_count = 0;
        });
    }

    fn output_node_ids(r_nodes: &[RuntimeNode]) -> Vec<NodeId> {
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
        r_node.terminal || r_node.output_values.is_none() || r_node.behavior == FuncBehavior::Active
    }

    fn collect_ordered_output_dependencies(
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
                    let output_node =
                        graph
                            .node_by_id(output_binding.output_node_id)
                            .ok_or_else(|| {
                                anyhow::Error::msg("Output binding references missing node")
                            })?;
                    let output_func =
                        func_lib.func_by_id(output_node.func_id).ok_or_else(|| {
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

    fn build_node_index(nodes: &[RuntimeNode]) -> HashMap<NodeId, usize> {
        let mut map = HashMap::with_capacity(nodes.len());
        for (index, node) in nodes.iter().enumerate() {
            let prev = map.insert(node.id, index);
            assert!(
                prev.is_none(),
                "Duplicate runtime node id detected: {:?}",
                node.id
            );
        }
        map
    }
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

        let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
        runtime_graph.next(&graph);

        assert_eq!(runtime_graph.nodes.len(), 5);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .total_binding_count,
            2
        );
        assert!(runtime_graph
            .nodes
            .iter()
            .all(|r_node| r_node.should_invoke));
        assert!(runtime_graph
            .nodes
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

        let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
        runtime_graph.next(&graph);

        assert_eq!(runtime_graph.nodes.len(), 5);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap_or_else(|| panic!("Runtime node {:?} missing", get_b_node_id))
                .total_binding_count,
            2
        );

        runtime_graph.next(&graph);

        assert_eq!(runtime_graph.nodes.len(), 5);
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

        let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
        runtime_graph.next(&graph);

        assert_eq!(runtime_graph.nodes.len(), 4);
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
