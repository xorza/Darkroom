use crate::function_graph::*;
use crate::runtime_graph::{RuntimeGraph, RuntimeNode};
use crate::workspace::Workspace;
use std::time::{Instant};
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

    pub fn run(&mut self, workspace: &Workspace) {
        let mut last_run = self.compute_nodes.clone();
        self.compute_nodes.clear();

        self.runtime_graph.run(&workspace.graph());

        for r_node in self.runtime_graph.nodes().clone() {
            let args = workspace.function_graph()
                .args_by_node_id(r_node.node_id())
                .collect::<Vec<&Arg>>();
            let input_args = args.iter().cloned()
                .filter(|arg| arg.direction == Direction::In)
                .collect::<Vec<&Arg>>();
            let output_args = args.iter().cloned()
                .filter(|arg| arg.direction == Direction::Out)
                .collect::<Vec<&Arg>>();

            let mut compute_node = ComputeNode::new(r_node.node_id());
            if let Some(existing_compute_node) = last_run.iter_mut()
                .find(|_node| _node.node_id() == r_node.node_id()) {
                compute_node.inputs = existing_compute_node.inputs.clone();
                compute_node.outputs = existing_compute_node.outputs.clone();

                assert_eq!(compute_node.inputs.len(), input_args.len());
                assert_eq!(compute_node.outputs.len(), output_args.len());
            } else {
                compute_node.inputs.resize(input_args.len(), 0);
                compute_node.outputs.resize(output_args.len(), 0);
            }

            if r_node.should_execute {
                for input_arg in input_args {
                    let input = workspace.graph().input_by_id(input_arg.input_output_id()).unwrap();
                    let output = workspace.graph().output_by_id(input.connected_output_id).unwrap();
                    let output_runtime_node = self.runtime_graph.node_by_id(output.node_id()).unwrap();

                    if output_runtime_node.should_execute || !r_node.has_outputs {
                        let output_compute_node = self.compute_nodes.iter().find(|node| node.node_id() == output.node_id()).unwrap();
                        let output_arg = workspace.function_graph().arg_by_input_output_id(output.id()).unwrap();

                        assert!(output_arg.direction == Direction::Out);
                        assert!(output_arg.data_type == input_arg.data_type);

                        compute_node.inputs[input_arg.index as usize] = output_compute_node.outputs[output_arg.index as usize];
                    }
                }

                let function = workspace.function_graph().function_by_node_id(r_node.node_id()).unwrap();

                let start = Instant::now();
                self.invoker.call(
                    &function.name,
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