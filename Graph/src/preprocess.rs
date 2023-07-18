use std::collections::HashSet;
use std::mem::take;

use crate::graph::*;
use crate::runtime_graph::{InvokeContext, RuntimeGraph, RuntimeNode};

#[derive(Default)]
pub struct Preprocess {}

impl Preprocess {
    pub fn run(&self, graph: &Graph, previous_runtime: &mut RuntimeGraph) -> RuntimeGraph {
        debug_assert!(graph.validate().is_ok());

        let mut r_nodes = self.gather_nodes(graph, previous_runtime);
        self.forward_pass(graph, &mut r_nodes);
        self.backward_pass(graph, &mut r_nodes);

        RuntimeGraph {
            nodes: r_nodes,
        }
    }


    fn gather_nodes(
        &self,
        graph: &Graph,
        previous_runtime: &mut RuntimeGraph,
    ) -> Vec<RuntimeNode>
    {
        let mut active_node_ids: Vec<NodeId> = graph
            .nodes()
            .iter()
            .filter_map(|node| {
                if node.is_output {
                    Some(node.id())
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

            node.inputs.iter()
                .for_each(|input| {
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

        let r_nodes: Vec<RuntimeNode> = active_node_ids.iter()
            .map(|&node_id| {
                let node = graph.node_by_id(node_id).unwrap();

                let prev_r_node = previous_runtime.node_by_id_mut(node_id);

                let (invoke_context, output_values) =
                    if let Some(prev_r_node) = prev_r_node {
                        assert_eq!(prev_r_node.output_binding_count.len(), node.outputs.len());
                        debug_assert_eq!(prev_r_node.name, node.name);

                        (
                            take(&mut prev_r_node.invoke_context),
                            prev_r_node.output_values.take()
                        )
                    } else {
                        (
                            InvokeContext::default(),
                            None
                        )
                    };


                let r_node = RuntimeNode {
                    node_id,
                    name: node.name.clone(),
                    is_output: node.is_output,
                    has_missing_inputs: false,
                    behavior: node.behavior,
                    should_execute: false,
                    should_cache_outputs: node.should_cache_outputs,
                    run_time: 0.0,
                    invoke_context,
                    output_values,
                    output_binding_count: vec![0; node.outputs.len()],
                    total_binding_count: 0,
                };

                r_node
            })
            .collect::<Vec<RuntimeNode>>();

        r_nodes
    }

    // in forward pass, mark active nodes and nodes with missing inputs
    // if node is passive, mark it for caching outputs
    fn forward_pass(&self,
                    graph: &Graph,
                    r_nodes: &mut Vec<RuntimeNode>,
    ) {
        for index in 0..r_nodes.len() {
            let mut r_node = take(&mut r_nodes[index]);
            let node = graph.node_by_id(r_node.node_id).unwrap();

            for input in node.inputs.iter() {
                match &input.binding {
                    Binding::None => {
                        r_node.has_missing_inputs |= input.is_required;
                    }
                    Binding::Const => {}
                    Binding::Output(output_binding) => {
                        let output_r_node = r_nodes[0..index].iter()
                            .find(|&p_node| p_node.node_id == output_binding.output_node_id)
                            .expect("Node not found among already processed ones");
                        if output_r_node.behavior == FunctionBehavior::Active {
                            r_node.behavior = FunctionBehavior::Active;
                        }
                        r_node.has_missing_inputs |= output_r_node.has_missing_inputs;
                    }
                }
            }

            if r_node.behavior == FunctionBehavior::Passive {
                r_node.should_cache_outputs = true;
            }
            r_nodes[index] = r_node;
        }
    }
    // in backward pass, mark active nodes without cached outputs for execution
    fn backward_pass(&self,
                     graph: &Graph,
                     r_nodes: &mut Vec<RuntimeNode>,
    ) {
        let mut active_node_ids: Vec<NodeId> = r_nodes.iter()
            .filter_map(|r_node| {
                if r_node.is_output {
                    Some(r_node.node_id)
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
            let r_node =
                r_nodes
                    .iter_mut()
                    .find(|r_node| r_node.node_id == node_id).unwrap();

            if r_node.is_output {
                r_node.should_execute = true;
            } else if r_node.output_values.is_none() {
                r_node.should_execute = true;
            } else if r_node.should_cache_outputs {
                r_node.should_execute = false;
            } else if r_node.behavior == FunctionBehavior::Active {
                r_node.should_execute = true;
            } else {
                r_node.should_execute = false;
            }

            if r_node.should_execute {
                node.inputs.iter()
                    .for_each(|input| {
                        if let Binding::Output(output_binding) = &input.binding {
                            active_node_ids.push(output_binding.output_node_id);
                            let output_r_node =
                                r_nodes
                                    .iter_mut()
                                    .find(|r_node| r_node.node_id == output_binding.output_node_id).unwrap();
                            output_r_node.increment_binding_count(output_binding.output_index);
                        }
                    });
            }
        }
    }
}
