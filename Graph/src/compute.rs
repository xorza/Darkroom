use crate::function_graph::{Argument, Direction, Function};
use crate::runtime_graph::RuntimeGraph;
use crate::workspace::Workspace;

pub struct ComputeNode {
    node_id: u32,
    pub inputs: Vec<i32>,
    pub outputs: Vec<i32>,
    pub run_time: f32,

}

pub struct Compute {
    runtime_graph: RuntimeGraph,
    compute_nodes: Vec<ComputeNode>,
}

impl Compute {
    pub fn new() -> Compute {
        Compute {
            runtime_graph: RuntimeGraph::new(),
            compute_nodes: Vec::new(),
        }
    }

    pub fn run(&mut self, workspace: &Workspace) {
        self.runtime_graph.run(&workspace.graph());

        let nodes_to_run
            = self.runtime_graph.nodes().iter()
            .filter(|node| node.should_execute);
        for r_node in nodes_to_run {
            let args = workspace.function_graph()
                .arguments_by_node_id(r_node.node_id())
                .collect::<Vec<&Argument>>();
            // let compute_node = self.get_node(r_node.node_id(), &args);
            //
            // for arg in args {
            //     if arg.direction == Direction::In {
            //         let input = workspace.graph().input_by_id(arg.input_output_id()).unwrap();
            //         let output = workspace.graph().output_by_id(input.connected_output_id).unwrap();
            //         let output_compute_node = self.compute_nodes.iter().find(|node| node.node_id == output.node_id()).unwrap();
            //         let output_arg = workspace.function_graph().argument_by_input_output_id(output.id()).unwrap();
            //
            //         assert!(output_arg.direction == Direction::Out);
            //         assert!(output_arg.data_type == arg.data_type);
            //         // assert!(output_compute_node.)
            //
            //         compute_node.inputs[arg.index as usize] = output_compute_node.outputs[output_arg.index as usize];
            //     }
            // }

            // let function = workspace.function_graph().function_by_node_id(r_node.node_id()).unwrap();
            // self.run_function(function, compute_node);
        }
    }

    fn run_function(self, function: &Function, compute_node: &mut ComputeNode) {
        match function.name.as_str() {
            "val0" => {
                compute_node.outputs[0] = 2;
            }
            "val1" => {
                compute_node.outputs[0] = 5;
            }
            "mult" => {
                compute_node.outputs[0] = compute_node.inputs[0] * compute_node.inputs[1];
            }
            "sum" => {
                compute_node.outputs[0] = compute_node.inputs[0] + compute_node.inputs[1];
            }
            "print" => {
                println!("{}", compute_node.inputs[0]);
            }
            _ => panic!("Unknown function: {}", function.name),
        }
    }

    fn get_node(&mut self, node_id: u32, args: &Vec<&Argument>) -> &mut ComputeNode {
        if let Some(node) = self.compute_nodes.iter_mut().find(|node| node.node_id == node_id) {
            return node;
        }

        // let mut node = ComputeNode::new(node_id);
        // let input_count = args.iter().filter(|arg| arg.direction == Direction::In).count();
        // let output_count = args.iter().filter(|arg| arg.direction == Direction::Out).count();
        // node.inputs.resize(input_count, 0);
        // node.outputs.resize(output_count, 0);
        //
        // self.compute_nodes.push(node);
        // return self.compute_nodes.last_mut().unwrap();

        panic!("Node not found: {}", node_id);
    }
}

impl ComputeNode {
    pub fn new(node_id: u32) -> ComputeNode {
        ComputeNode {
            node_id,
            run_time: 0.0,
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub fn node_id(&self) -> u32 {
        self.node_id
    }
}