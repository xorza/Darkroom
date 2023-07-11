use std::collections::HashSet;

use crate::graph::*;
use crate::runtime_graph::{RuntimeGraph, RuntimeNode, RuntimeOutput};

#[derive(Default)]
pub struct Preprocess {}


#[derive(Default, Clone)]
struct Edge {
    output_node_id: NodeId,
    output_index: Option<u32>,
    input_node_id: NodeId,
    input_index: u32,
    has_missing_inputs: bool,
    is_output: bool,
}

impl Preprocess {
    pub fn run(&self, graph: &Graph) -> RuntimeGraph {
        self.run1(graph, &HashSet::new())
    }
    pub fn run1(&self, graph: &Graph, cached_nodes: &HashSet<NodeId>) -> RuntimeGraph {
        debug_assert!(graph.validate().is_ok());

        let edges = self.gather_edges(graph, cached_nodes);
        let r_nodes = self.gather_nodes(graph, edges, cached_nodes);
        let r_nodes = self.process_behavior_and_inputs(graph, r_nodes);

        RuntimeGraph {
            nodes: r_nodes,
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
        all_edges: Vec<Edge>,
        caches_nodes: &HashSet<NodeId>)
        -> Vec<RuntimeNode>
    {
        let mut r_nodes: Vec<RuntimeNode> = Vec::new();
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

                let behavior =
                    if caches_nodes.contains(&node_id) {
                        FunctionBehavior::Passive
                    } else {
                        node.behavior
                    };

                r_nodes.push(RuntimeNode {
                    node_id,
                    outputs: vec![RuntimeOutput::default(); node.outputs.len()],
                    is_output,
                    has_missing_inputs: false,
                    behavior,
                    name: node.name.clone(),
                });
                if let Some(output_index) = edge.output_index {
                    r_nodes.last_mut().unwrap()
                        .outputs[output_index as usize].binding_count += 1;
                }
            } else if let Some(output_index) = edge.output_index {
                r_nodes.iter_mut()
                    .find(|p_node| p_node.node_id == node_id)
                    .unwrap()
                    .outputs[output_index as usize].binding_count += 1;
            }
        }

        r_nodes.reverse();

        r_nodes
    }

    fn process_behavior_and_inputs(&self, graph: &Graph, mut r_nodes: Vec<RuntimeNode>) -> Vec<RuntimeNode> {
        for i in 0..r_nodes.len() {
            let mut p_node = std::mem::take(&mut r_nodes[i]);
            let node = graph.node_by_id(p_node.node_id).unwrap();

            {
                let processed_nodes = &mut r_nodes[0..i];

                for (_index, input) in node.inputs.iter().enumerate() {
                    match &input.binding {
                        Binding::None => {
                            p_node.has_missing_inputs |= input.is_required;
                        }
                        Binding::Const => {}
                        Binding::Output(output_binding) => {
                            let output_p_node = processed_nodes
                                .iter()
                                .find(|p_node| p_node.node_id == output_binding.output_node_id)
                                .expect("Node not found among already processed ones");

                            if output_p_node.behavior == FunctionBehavior::Active {
                                p_node.behavior = FunctionBehavior::Active;
                            }

                            p_node.has_missing_inputs |= output_p_node.has_missing_inputs;
                        }
                    }
                }
            }

            r_nodes[i] = p_node;
        }

        r_nodes
    }
}
