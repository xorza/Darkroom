use crate::function_graph::*;
use crate::runtime_graph::RuntimeGraph;
use crate::workspace::Workspace;

#[derive(Clone)]
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
        let mut last_run = self.compute_nodes.clone();
        self.compute_nodes.clear();

        self.runtime_graph.run(&workspace.graph());

        let nodes_to_run
            = self.runtime_graph.nodes().iter()
            .filter(|_node| _node.should_execute)
            .map(|_node| _node.node_id())
            .collect::<Vec<u32>>();
        for node_id in nodes_to_run {
            let args = workspace.function_graph()
                .arguments_by_node_id(node_id)
                .collect::<Vec<&Argument>>();


            let input_count = args.iter().filter(|arg| arg.direction == Direction::In).count();
            let output_count = args.iter().filter(|arg| arg.direction == Direction::Out).count();

            let mut compute_node = ComputeNode::new(node_id);
            if let Some(existing_compute_node) =
                last_run.iter_mut()
                    .find(|node| node.node_id == node_id) {
                compute_node.outputs = existing_compute_node.outputs.clone();
                compute_node.inputs = existing_compute_node.inputs.clone();

                assert_eq!(compute_node.inputs.len(), input_count);
                assert_eq!(compute_node.outputs.len(), output_count);
            } else {
                compute_node.inputs.resize(input_count, 0);
                compute_node.outputs.resize(output_count, 0);
            }

            for arg in args {
                if arg.direction == Direction::In {
                    let input = workspace.graph().input_by_id(arg.input_output_id()).unwrap();
                    let output = workspace.graph().output_by_id(input.connected_output_id).unwrap();
                    let output_compute_node = self.compute_nodes.iter().find(|node| node.node_id == output.node_id()).unwrap();
                    let output_arg = workspace.function_graph().argument_by_input_output_id(output.id()).unwrap();

                    assert!(output_arg.direction == Direction::Out);
                    assert!(output_arg.data_type == arg.data_type);
                    // assert!(output_compute_node.)

                    compute_node.inputs[arg.index as usize] = output_compute_node.outputs[output_arg.index as usize];
                }
            }

            let function = workspace.function_graph().function_by_node_id(node_id).unwrap();
            self.run_function(function, &mut compute_node);


            self.compute_nodes.push(compute_node);
        }
    }

    fn run_function(&self, function: &Function, compute_node: &mut ComputeNode) {
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