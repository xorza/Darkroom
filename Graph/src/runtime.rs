use std::mem;
use std::time::Instant;
use crate::common::is_debug;
use crate::graph::*;
use crate::invoke::{Args, Invoker};

#[derive(Clone)]
pub struct RuntimeNode {
    pub node_id: u32,
    index: usize,

    pub has_missing_inputs: bool,
    pub has_updated_bindings: bool,
    pub binding_behavior: BindingBehavior,
    pub should_execute: bool,
    pub has_outputs: bool,
    pub inputs: Args,
    pub outputs: Args,
    pub run_time: f64,
    pub execution_index: u32,
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
    fn node_by_id_mut(&mut self, node_id: u32) -> &mut RuntimeNode {
        self.nodes.iter_mut().find(|node| node.node_id() == node_id).unwrap()
    }
    pub fn prepare(&mut self, graph: &Graph) {
        assert!(graph.validate());

        let mut last_run = self.nodes.clone();
        self.nodes.clear();
        self.nodes = graph.nodes().iter().enumerate().map(
            |(i, node)| -> RuntimeNode{
                RuntimeNode::new(i, node, &mut last_run)
            }
        ).collect();

        let order = self.traverse_backward(graph);
        self.traverse_forward(graph, order);
    }


    fn traverse_backward(&mut self, graph: &Graph) -> Vec<usize> {
        let active_nodes = graph.nodes().iter().filter(|node| node.is_output);

        let mut order: Vec<usize> = Vec::new();

        for node in active_nodes {
            let mut rnode = self.node_by_id_mut(node.id());
            rnode.has_outputs = true;
            rnode.binding_behavior = BindingBehavior::Always;

            let index = rnode.index;
            order.push(index);
        }


        let mut i: usize = 0;
        while i < order.len() {
            let index = order[i];
            let mut rnode = self.nodes[index].clone();
            let node = graph.node_by_id(rnode.node_id()).unwrap();

            for input in node.inputs.iter() {
                if input.binding.is_none() {
                    rnode.has_missing_inputs = true;
                    continue;
                }

                let binding = input.binding.as_ref().unwrap();
                let output_node = graph.node_by_id(binding.node_id()).unwrap();
                let output_rnode: &mut RuntimeNode = self.node_by_id_mut(output_node.id());

                assert_eq!(output_rnode.inputs.len(), output_node.inputs.len());
                assert_eq!(output_rnode.outputs.len(), output_node.outputs.len());

                if rnode.binding_behavior == BindingBehavior::Always
                    && binding.behavior == BindingBehavior::Always {
                    output_rnode.binding_behavior = BindingBehavior::Always;
                }

                if !order.contains(&output_rnode.index) {
                    order.push(output_rnode.index);
                }
            }

            self.nodes[index] = rnode;
            i += 1;
        }

        order.reverse();

        order
    }
    fn traverse_forward(&mut self, graph: &Graph, order: Vec<usize>) {
        let mut i: usize = 0;
        while i < order.len() {
            let index = order[i];
            let mut rnode = self.nodes[index].clone();
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

                        if binding.behavior == BindingBehavior::Always {
                            let output_rnode = self.nodes.iter().find(|_node| _node.node_id() == binding.node_id()).unwrap();
                            if output_rnode.should_execute {
                                rnode.has_updated_bindings = true;
                            }
                        }
                    }
                }
            }

            rnode.should_execute = self.should_execute(node, &rnode);

            self.nodes[index] = rnode;
            i += 1;
        }
    }
    fn should_execute(&self, node: &Node, rnode: &RuntimeNode) -> bool {
        if node.is_output
        { return true; }

        if rnode.has_missing_inputs
        { return false; }

        if !rnode.has_outputs
        { return true; }

        if rnode.binding_behavior == BindingBehavior::Once
        { return false; }

        if node.behavior == NodeBehavior::Active
        { return true; }

        if rnode.has_updated_bindings
        { return true; }

        return false;
    }

    pub fn run(&mut self, graph: &Graph, invoker: &dyn Invoker) {
        invoker.start();
        let mut execution_index: u32 = 0;

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
                invoker.call(&node.name, rnode.node_id(), &rnode.inputs, &mut rnode.outputs);
                rnode.run_time = start.elapsed().as_secs_f64();
                rnode.has_outputs = true;
                rnode.execution_index = execution_index;
                execution_index += 1;

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


    pub fn new(index: usize, node: &Node, last_run: &mut Vec<RuntimeNode>) -> RuntimeNode {
        let mut result = RuntimeNode {
            index,
            node_id: node.id(),
            has_missing_inputs: false,
            binding_behavior: BindingBehavior::Once,
            should_execute: true,
            has_outputs: false,
            has_updated_bindings: false,
            inputs: Vec::new(),
            outputs: Vec::new(),
            run_time: 0.0,
            execution_index: 0,
        };

        let existing_rnode
            = last_run.iter_mut().find(|_last_run_node| _last_run_node.node_id() == node.id());
        if let Some(existing_rnode) = existing_rnode {
            result.has_outputs = existing_rnode.has_outputs;
            result.inputs = mem::replace(&mut existing_rnode.inputs, Args::new());
            result.outputs = mem::replace(&mut existing_rnode.outputs, Args::new());
        } else {
            result.inputs.resize(node.inputs.len(), 0);
            result.outputs.resize(node.outputs.len(), 0);
        }

        return result;
    }
}