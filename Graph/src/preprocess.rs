use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::graph::*;

#[derive(Default)]
pub struct Preprocess {}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct PreprocessInput {}

#[derive(Default, Clone, Debug, Serialize, Deserialize)]
pub struct PreprocessOutput {
    pub binding_count: u32,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct PreprocessNode {
    node_id: NodeId,

    pub name: String,

    pub inputs: Vec<PreprocessInput>,
    pub outputs: Vec<PreprocessOutput>,
    pub is_output: bool,

    pub has_missing_inputs: bool,
    pub behavior: FunctionBehavior,
}

#[derive(Default, Clone)]
struct Edge {
    output_node_id: NodeId,
    output_index: Option<u32>,
    input_node_id: NodeId,
    input_index: u32,
    has_missing_inputs: bool,
    is_output: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct PreprocessInfo {
    pub nodes: Vec<PreprocessNode>,
}

impl Preprocess {
    pub fn run(&self, graph: &Graph) -> PreprocessInfo {
        self.run1(graph, &HashSet::new())
    }
    pub fn run1(&self, graph: &Graph, cached_nodes: &HashSet<NodeId>) -> PreprocessInfo {
        assert!(graph.validate().is_ok());

        let edges = self.gather_edges(graph, cached_nodes);
        let pp_nodes = self.gather_nodes(graph, edges);
        let pp_nodes = self.process_behavior_and_inputs(graph, pp_nodes);

        PreprocessInfo {
            nodes: pp_nodes,
        }
    }

    fn gather_edges(&self, graph: &Graph, caches_nodes: &HashSet<NodeId>) -> Vec<Edge> {
        let mut all_edges = graph.nodes()
            .iter()
            .filter(|node| node.is_output)
            .map(|node| {
                Edge {
                    output_node_id: node.id(),
                    output_index: None,
                    input_node_id: NodeId::nil(),
                    input_index: 0,
                    has_missing_inputs: false,
                    is_output: true,
                }
            })
            .collect::<Vec<Edge>>();

        let mut node_ids: HashSet<NodeId> = HashSet::new();

        let mut i: usize = 0;
        while i < all_edges.len() {
            i += 1;
            let i = i - 1;

            let edge = &all_edges[i];
            if !node_ids.insert(edge.output_node_id) {
                continue;
            }
            if caches_nodes.contains(&edge.output_node_id) {
                continue;
            }

            let node = graph
                .node_by_id(edge.output_node_id)
                .expect("Node not found");
            for (input_index, input) in node.inputs.iter().enumerate() {
                if let Binding::Output(output_binding) = &input.binding {
                    assert_ne!(output_binding.output_node_id, node.id());

                    all_edges.push(Edge {
                        output_node_id: output_binding.output_node_id,
                        output_index: Some(output_binding.output_index),
                        input_node_id: node.id(),
                        input_index: input_index as u32,
                        has_missing_inputs: false,
                        is_output: false,
                    });
                }
            }
        }

        all_edges
    }
    fn gather_nodes(
        &self,
        graph: &Graph,
        all_edges: Vec<Edge>)
        -> Vec<PreprocessNode>
    {
        let mut pp_nodes: Vec<PreprocessNode> = Vec::new();
        let mut node_ids: HashSet<NodeId> = HashSet::new();

        for edge in all_edges.iter() {
            let node_id = edge.output_node_id;

            if node_ids.insert(node_id) {
                let node = graph.node_by_id(node_id).unwrap();

                let node_output_edges = all_edges.iter()
                    .filter(|&edge| edge.output_node_id == node_id)
                    .collect::<Vec<&Edge>>();

                let is_output = node_output_edges.iter()
                    .any(|&edge| edge.is_output);

                pp_nodes.push(PreprocessNode {
                    node_id,
                    inputs: vec![PreprocessInput::default(); node.inputs.len()],
                    outputs: vec![PreprocessOutput::default(); node.outputs.len()],
                    is_output,
                    has_missing_inputs: false,
                    behavior: node.behavior,
                    name: node.name.clone(),
                });
                if let Some(output_index) = edge.output_index {
                    pp_nodes.last_mut().unwrap()
                        .outputs[output_index as usize].binding_count += 1;
                }
            } else if let Some(output_index) = edge.output_index {
                pp_nodes.iter_mut()
                    .find(|pp_node| pp_node.node_id == node_id)
                    .unwrap()
                    .outputs[output_index as usize].binding_count += 1;
            }
        }

        pp_nodes.reverse();

        pp_nodes
    }

    fn process_behavior_and_inputs(&self, graph: &Graph, mut pp_nodes: Vec<PreprocessNode>) -> Vec<PreprocessNode> {
        for i in 0..pp_nodes.len() {
            let mut pp_node = std::mem::take(&mut pp_nodes[i]);
            let node = graph.node_by_id(pp_node.node_id).unwrap();

            {
                let processed_nodes = &mut pp_nodes[0..i];

                for (index, input) in node.inputs.iter().enumerate() {
                    let _pp_input = &mut pp_node.inputs[index];

                    match &input.binding {
                        Binding::None => {
                            pp_node.has_missing_inputs |= input.is_required;
                        }
                        Binding::Const => {}
                        Binding::Output(output_binding) => {
                            let output_pp_node = processed_nodes
                                .iter()
                                .find(|pp_node| pp_node.node_id == output_binding.output_node_id)
                                .expect("Node not found among already processed ones");

                            if output_pp_node.behavior == FunctionBehavior::Active {
                                pp_node.behavior = FunctionBehavior::Active;
                            }

                            pp_node.has_missing_inputs |= output_pp_node.has_missing_inputs;
                        }
                    }
                }
            }

            pp_nodes[i] = pp_node;
        }

        pp_nodes
    }
}

impl PreprocessNode {
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }
}

impl PreprocessInfo {
    pub fn node_by_name(&self, name: &str) -> Option<&PreprocessNode> {
        self.nodes.iter().find(|&pp_node| pp_node.name == name)
    }

    pub fn node_by_id(&self, node_id: NodeId) -> &PreprocessNode {
        self.nodes.iter()
            .find(|&pp_node| pp_node.node_id == node_id)
            .unwrap()
    }
    pub fn node_by_id_mut(&mut self, node_id: NodeId) -> &mut PreprocessNode {
        self.nodes.iter_mut()
            .find(|pp_node| pp_node.node_id == node_id)
            .unwrap()
    }
}
