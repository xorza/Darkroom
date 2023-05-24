use crate::runtime_graph::{RuntimeGraph, RuntimeNode};
use std::time::{Instant};
use crate::common::is_debug;
use crate::graph::Graph;
use crate::invoke::{Args, Invoker, LambdaInvoker};

#[derive(Clone)]
pub struct ComputeNode {
    node_id: u32,
    pub inputs: Args,
    pub outputs: Args,
    pub run_time: f32,

}

pub struct Compute {
    runtime_graph: RuntimeGraph,
    compute_nodes: Vec<ComputeNode>,
    pub invoker: Box<dyn Invoker>,
}

impl Compute {
    pub fn new() -> Compute {
        Compute {
            runtime_graph: RuntimeGraph::new(),
            compute_nodes: Vec::new(),
            invoker: Box::new(LambdaInvoker::new()),
        }
    }


    pub fn compute_nodes(&self) -> &Vec<ComputeNode> {
        &self.compute_nodes
    }
    pub fn runtime_nodes(&self) -> &Vec<RuntimeNode> {
        &self.runtime_graph.nodes()
    }

    pub fn run(&mut self, graph: &Graph) {
        let mut last_run = self.compute_nodes.clone();
        self.compute_nodes.clear();

        self.runtime_graph.run(graph);

        for r_node in self.runtime_graph.nodes().clone() {
            let node = graph.node_by_id(r_node.node_id()).unwrap();

            let mut compute_node = ComputeNode::new(r_node.node_id());
            if let Some(existing_compute_node) = last_run.iter_mut()
                .find(|_node| _node.node_id() == r_node.node_id()) {
                compute_node.inputs = existing_compute_node.inputs.clone();
                compute_node.outputs = existing_compute_node.outputs.clone();
            } else {
                compute_node.inputs.resize(node.inputs.len(), 0);
                compute_node.outputs.resize(node.outputs.len(), 0);
            }

            assert_eq!(compute_node.inputs.len(), node.inputs.len());
            assert_eq!(compute_node.outputs.len(), node.outputs.len());

            if r_node.should_execute {
                for (i, input) in node.inputs.iter().enumerate() {
                    let binding = input.binding.as_ref().unwrap();
                    let output_node = graph.node_by_id(binding.node_id).unwrap();
                    let output_runtime_node = self.runtime_graph.node_by_id(binding.node_id).unwrap();

                    if output_runtime_node.should_execute || !r_node.has_outputs {
                        if is_debug() {
                            let output_arg = output_node.outputs.get(binding.output_index).unwrap();
                            assert!(output_arg.data_type == input.data_type);
                        }

                        let output_compute_node = self.compute_nodes.iter().find(|_node| _node.node_id == binding.node_id).unwrap();
                        compute_node.inputs[i] = output_compute_node.outputs[binding.output_index];
                    }
                }

                let start = Instant::now();
                self.invoker.call(
                    &node.name,
                    r_node.node_id(),
                    &compute_node.inputs,
                    &mut compute_node.outputs);
                compute_node.run_time = start.elapsed().as_secs_f32();
            }

            self.compute_nodes.push(compute_node);
        }

        self.invoker.finish();
    }
}

impl ComputeNode {
    pub fn new(node_id: u32) -> ComputeNode {
        ComputeNode {
            node_id,
            run_time: 0.0,
            inputs: Args::new(),
            outputs: Args::new(),
        }
    }

    pub fn node_id(&self) -> u32 {
        self.node_id
    }
}