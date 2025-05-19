use std::collections::HashSet;
use std::mem::take;

use serde::{Deserialize, Serialize};

use crate::data::DynamicValue;
use crate::function::FuncLib;
use crate::function::FuncBehavior;
use crate::graph::{Binding, Graph, NodeId};
use crate::invoke::InvokeCache;

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct RuntimeNode {
    id: NodeId,

    pub is_output: bool,
    pub has_missing_inputs: bool,
    pub behavior: FuncBehavior,
    pub should_cache_outputs: bool,
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
}

impl RuntimeNode {
    pub fn id(&self) -> NodeId {
        self.id
    }

    pub(crate) fn decrement_current_binding_count(&mut self, output_index: u32) {
        assert!(self.output_binding_count[output_index as usize] >= 1);
        assert!(self.total_binding_count >= 1);

        self.output_binding_count[output_index as usize] -= 1;
        self.total_binding_count -= 1;
    }
}

impl RuntimeGraph {
    pub fn new(graph: &Graph, func_lib: &FuncLib) -> Self {
        let runtime_graph = Self::run(graph, func_lib, &mut RuntimeGraph::default());

        runtime_graph
    }

    pub fn node_by_id(&self, node_id: NodeId) -> Option<&RuntimeNode> {
        self.nodes.iter().find(|&p_node| p_node.id == node_id)
    }
    pub fn node_by_id_mut(&mut self, node_id: NodeId) -> Option<&mut RuntimeNode> {
        self.nodes.iter_mut().find(|p_node| p_node.id == node_id)
    }

    pub fn next(&mut self, graph: &Graph) {
        Self::backward_pass(graph, &mut self.nodes);
    }
}

impl RuntimeGraph {
    fn run(graph: &Graph, func_lib: &FuncLib, previous_runtime: &mut RuntimeGraph) -> RuntimeGraph {
        debug_assert!(graph.validate().is_ok());

        let mut r_nodes = Self::gather_nodes(graph, func_lib, previous_runtime);
        Self::forward_pass(graph, func_lib, &mut r_nodes);

        RuntimeGraph { nodes: r_nodes }
    }

    fn gather_nodes(
        graph: &Graph,
        func_lib: &FuncLib,
        previous_runtime: &mut RuntimeGraph,
    ) -> Vec<RuntimeNode> {
        let mut active_node_ids: Vec<NodeId> = graph
            .nodes()
            .iter()
            .filter_map(|node| if node.is_output { Some(node.id) } else { None })
            .collect();

        let mut index = 0;
        while index < active_node_ids.len() {
            index += 1;
            let index = index - 1;

            let node_id = active_node_ids[index];
            let node = graph.node_by_id(node_id).unwrap();

            node.inputs.iter().for_each(|input| {
                if let Binding::Output(output_binding) = &input.binding {
                    active_node_ids.push(output_binding.output_node_id);
                }
            });
        }

        active_node_ids.reverse();
        {
            let mut set = HashSet::new();
            active_node_ids.retain(|&x| set.insert(x));
        }

        let r_nodes: Vec<RuntimeNode> = active_node_ids
            .iter()
            .map(|&node_id| {
                let node = graph.node_by_id(node_id).unwrap();
                let node_info = func_lib.func_by_id(node.func_id).unwrap();

                let prev_r_node = previous_runtime.node_by_id_mut(node_id);

                let (invoke_context, output_values) = if let Some(prev_r_node) = prev_r_node {
                    assert_eq!(
                        prev_r_node.output_binding_count.len(),
                        node_info.outputs.len()
                    );

                    (
                        take(&mut prev_r_node.cache),
                        prev_r_node.output_values.take(),
                    )
                } else {
                    (InvokeCache::default(), None)
                };

                let r_node = RuntimeNode {
                    id: node_id,
                    is_output: node.is_output,
                    has_missing_inputs: false,
                    behavior: node_info.behavior,
                    should_cache_outputs: node.cache_outputs,
                    run_time: 0.0,
                    should_invoke: false,
                    cache: invoke_context,
                    output_values,

                    output_binding_count: vec![0; node_info.outputs.len()],
                    total_binding_count: 0,
                };

                r_node
            })
            .collect::<Vec<RuntimeNode>>();

        r_nodes
    }

    // in forward pass, mark active nodes and nodes with missing inputs
    // if node is passive, mark it for caching outputs
    fn forward_pass(graph: &Graph, func_lib: &FuncLib, r_nodes: &mut [RuntimeNode]) {
        for index in 0..r_nodes.len() {
            let mut r_node = take(&mut r_nodes[index]);
            let node = graph.node_by_id(r_node.id).unwrap();
            let node_info = func_lib.func_by_id(node.func_id).unwrap();

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
                r_node.should_cache_outputs = true;
            }
            r_nodes[index] = r_node;
        }
    }

    // in backward pass, mark active nodes without cached outputs for execution
    fn backward_pass(graph: &Graph, r_nodes: &mut [RuntimeNode]) {
        r_nodes.iter_mut().for_each(|r_node| {
            r_node.should_invoke = false;
            r_node.output_binding_count.fill(0);
            r_node.total_binding_count = 0;
        });

        let mut active_node_ids: Vec<NodeId> = r_nodes
            .iter()
            .filter_map(|r_node| {
                if r_node.is_output {
                    Some(r_node.id)
                } else {
                    None
                }
            })
            .collect();

        let mut index = 0;
        while index < active_node_ids.len() {
            index += 1;
            let index = index - 1;

            let node_id = active_node_ids[index];
            let node = graph.node_by_id(node_id).unwrap();
            let r_node = r_nodes
                .iter_mut()
                .find(|r_node| r_node.id == node_id)
                .unwrap();

            let is_active = Self::is_active(r_node);
            r_node.should_invoke = is_active && !r_node.has_missing_inputs;

            node.inputs.iter().for_each(|input| {
                if let Binding::Output(output_binding) = &input.binding {
                    if is_active {
                        active_node_ids.push(output_binding.output_node_id);
                    }
                    let output_r_node = r_nodes
                        .iter_mut()
                        .find(|r_node| r_node.id == output_binding.output_node_id)
                        .unwrap();
                    output_r_node.output_binding_count[output_binding.output_index as usize] += 1;
                    output_r_node.total_binding_count += 1;
                }
            });
        }
    }

    #[allow(clippy::needless_bool)]
    fn is_active(r_node: &RuntimeNode) -> bool {
        if r_node.is_output {
            true
        } else if r_node.output_values.is_none() {
            true
        } else if r_node.behavior == FuncBehavior::Active {
            true
        } else {
            false
        }
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

        let get_b_node_id = graph.node_by_name("get_b").unwrap().id;

        let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
        runtime_graph.next(&graph);

        assert_eq!(runtime_graph.nodes.len(), 5);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap()
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

        let get_b_node_id = graph.node_by_name("get_b").unwrap().id;

        let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
        runtime_graph.next(&graph);

        assert_eq!(runtime_graph.nodes.len(), 5);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap()
                .total_binding_count,
            2
        );

        runtime_graph.next(&graph);

        assert_eq!(runtime_graph.nodes.len(), 5);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap()
                .total_binding_count,
            2
        );

        Ok(())
    }

    #[test]
    fn missing_input() -> anyhow::Result<()> {
        let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;
        let func_lib = FuncLib::from_yaml_file("../test_resources/test_funcs.yml")?;

        let get_b_node_id = graph.node_by_name("get_b").unwrap().id;
        let sum_node_id = graph.node_by_name("sum").unwrap().id;
        let mult_node_id = graph.node_by_name("mult").unwrap().id;
        let print_node_id = graph.node_by_name("print").unwrap().id;

        graph.node_by_name_mut("sum").unwrap().inputs[0].binding = Binding::None;

        let mut runtime_graph = RuntimeGraph::new(&graph, &func_lib);
        runtime_graph.next(&graph);

        assert_eq!(runtime_graph.nodes.len(), 4);
        assert_eq!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap()
                .total_binding_count,
            2
        );

        assert!(
            runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap()
                .should_invoke
        );
        assert!(!runtime_graph.node_by_id(sum_node_id).unwrap().should_invoke);
        assert!(
            !runtime_graph
                .node_by_id(mult_node_id)
                .unwrap()
                .should_invoke
        );
        assert!(
            !runtime_graph
                .node_by_id(print_node_id)
                .unwrap()
                .should_invoke
        );

        assert!(
            !runtime_graph
                .node_by_id(get_b_node_id)
                .unwrap()
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .node_by_id(sum_node_id)
                .unwrap()
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .node_by_id(mult_node_id)
                .unwrap()
                .has_missing_inputs
        );
        assert!(
            runtime_graph
                .node_by_id(print_node_id)
                .unwrap()
                .has_missing_inputs
        );

        let _yaml = serde_yml::to_string(&runtime_graph)?;

        Ok(())
    }
}
