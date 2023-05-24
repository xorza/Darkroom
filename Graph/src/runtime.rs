use std::time::Instant;
use crate::common::is_debug;
use crate::graph::*;
use crate::invoke::{Args, Invoker};

#[derive(Clone)]
pub struct RuntimeNode {
    node_id: u32,

    pub has_missing_inputs: bool,
    pub binding_behavior: BindingBehavior,
    pub should_execute: bool,
    pub has_outputs: bool,

    pub inputs: Args,
    pub outputs: Args,
    pub run_time: f32,
}

pub struct Runtime {
    nodes: Vec<RuntimeNode>,
}

impl Runtime {
    pub fn new() -> Runtime {
        Runtime {
            nodes: Vec::new(),
        }
    }

    pub fn nodes(&self) -> &Vec<RuntimeNode> {
        &self.nodes
    }
    pub fn node_by_id(&self, node_id: u32) -> Option<&RuntimeNode> {
        self.nodes.iter().find(|node| node.node_id() == node_id)
    }
    pub fn prepare(&mut self, graph: &Graph) {
        assert!(graph.validate());

        let last_run = self.nodes.clone();
        self.nodes.clear();

        self.traverse_backward(graph, last_run);
        self.traverse_forward(graph);
    }


    fn traverse_backward(&mut self, graph: &Graph, last_run: Vec<RuntimeNode>) {
        let active_nodes = graph.nodes().iter().filter(|node| node.is_output);
        for node in active_nodes {
            let mut rnode =
                RuntimeNode::create_for_output_node(node);

            if let Some(existing_rnode)
                = last_run.iter().find(|_node| _node.node_id() == node.id()) {
                rnode.inputs = existing_rnode.inputs.clone();
            }

            self.nodes.push(rnode);
        }

        let mut i: usize = 0;
        while i < self.nodes.len() {
            let mut rnode = self.nodes[i].clone();
            let node = graph.node_by_id(rnode.node_id()).unwrap();

            for input in &node.inputs {
                match &input.binding {
                    None => { rnode.has_missing_inputs = true; }
                    Some(binding) => {
                        let output_node = graph.node_by_id(binding.node_id()).unwrap();

                        let mut output_rnode: &mut RuntimeNode;
                        if let Some(existing_output_rnode)
                            = self.nodes.iter_mut().find(|_node| _node.node_id() == output_node.id()) {
                            output_rnode = existing_output_rnode;
                        } else {
                            self.nodes.push(RuntimeNode::new(output_node.id()));
                            output_rnode = self.nodes.last_mut().unwrap();
                            if let Some(last_run_node) = last_run.iter().find(|_last_run_node| _last_run_node.node_id() == output_node.id()) {
                                output_rnode.has_outputs = last_run_node.has_outputs;
                                output_rnode.inputs = last_run_node.inputs.clone();
                                output_rnode.outputs = last_run_node.outputs.clone();

                                assert_eq!(output_rnode.inputs.len(), output_node.inputs.len());
                                assert_eq!(output_rnode.outputs.len(), output_node.outputs.len());
                            } else {
                                output_rnode.has_outputs = false;
                                output_rnode.inputs.resize(output_node.inputs.len(), 0);
                                output_rnode.outputs.resize(output_node.outputs.len(), 0);
                            }
                        }

                        if rnode.binding_behavior == BindingBehavior::Always
                            && binding.behavior == BindingBehavior::Always {
                            output_rnode.binding_behavior = BindingBehavior::Always;
                        }
                    }
                }
            }

            self.nodes[i] = rnode;
            i += 1;
        }

        self.nodes.reverse();
    }
    fn traverse_forward(&mut self, graph: &Graph) {
        let mut i: usize = 0;
        while i < self.nodes.len() {
            let mut rnode = self.nodes[i].clone();
            let node = graph.node_by_id(rnode.node_id()).unwrap();

            for input in node.inputs.iter() {
                match input.binding.as_ref() {
                    None => {
                        if input.is_required {
                            rnode.has_missing_inputs = true;
                        }
                    }
                    Some(binding) => {
                        let output_rnode = self.nodes.iter().find(|_node| _node.node_id() == binding.node_id()).unwrap();
                        assert_eq!(output_rnode.has_outputs || output_rnode.should_execute, true);
                        if output_rnode.has_missing_inputs {
                            rnode.has_missing_inputs = true;
                        }
                    }
                }
            }

            rnode.should_execute = self.should_execute(node, &rnode);
            self.nodes[i] = rnode;

            i += 1;
        }
    }
    fn should_execute(&self, node: &Node, rnode: &RuntimeNode) -> bool {
        if node.is_output {
            return true;
        }

        if rnode.has_missing_inputs {
            return false;
        }

        if !rnode.has_outputs {
            return true;
        }

        if rnode.binding_behavior == BindingBehavior::Once {
            return false;
        }

        if node.behavior == NodeBehavior::Active {
            return true;
        }

        for input in node.inputs.iter() {
            match &input.binding {
                None => {
                    debug_assert_eq!(input.is_required, false);
                }
                Some(binding) => {
                    if binding.behavior == BindingBehavior::Always {
                        let output_rnode = self.nodes.iter().find(|_node| _node.node_id() == binding.node_id()).unwrap();
                        if output_rnode.should_execute {
                            return true;
                        }
                    }
                }
            }
        }

        return false;
    }

    pub fn run(&mut self, graph: &Graph, invoker: &dyn Invoker) {
        invoker.start();

        for i in 0..self.nodes().len() {
            let mut rnode = self.nodes[i].clone();
            let node = graph.node_by_id(rnode.node_id()).unwrap();

            if rnode.should_execute {
                for (i, input) in node.inputs.iter().enumerate() {
                    let binding = input.binding.as_ref().unwrap();
                    let output_runtime_node = self.node_by_id(binding.node_id()).unwrap();

                    assert_eq!(output_runtime_node.has_outputs, true);

                    if output_runtime_node.should_execute {
                        if is_debug() {
                            let output_node = graph.node_by_id(binding.node_id()).unwrap();
                            let output_arg = output_node.outputs.get(binding.output_index()).unwrap();
                            assert!(output_arg.data_type == input.data_type);
                        }

                        rnode.inputs[i] = output_runtime_node.outputs[binding.output_index()];
                    }
                }

                let start = Instant::now();
                invoker.call(
                    &node.name,
                    rnode.node_id(),
                    &rnode.inputs,
                    &mut rnode.outputs);
                rnode.run_time = start.elapsed().as_secs_f32();
                rnode.has_outputs = true;

                self.nodes[i] = rnode;
            }
        }


        invoker.finish();
    }
}

impl RuntimeNode {
    pub fn node_id(&self) -> u32 {
        self.node_id
    }

    pub fn create_for_output_node(node: &Node) -> RuntimeNode {
        let mut result = RuntimeNode {
            node_id: node.id(),
            has_missing_inputs: false,
            binding_behavior: BindingBehavior::Always,
            should_execute: true,
            has_outputs: true,
            inputs: Vec::new(),
            outputs: Vec::new(),
            run_time: 0.0,
        };

        result.inputs.resize(node.inputs.len(), 0);
        return result;
    }

    pub fn new(node_id: u32) -> RuntimeNode {
        RuntimeNode {
            node_id,
            has_missing_inputs: false,
            binding_behavior: BindingBehavior::Once,
            should_execute: true,
            has_outputs: false,
            inputs: Vec::new(),
            outputs: Vec::new(),
            run_time: 0.0,
        }
    }
}