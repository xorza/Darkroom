use std::collections::HashSet;
use std::mem;
use std::time::Instant;
use crate::common::is_debug;
use crate::graph::*;
use crate::invoke::{Args, Invoker};

#[derive(Clone, Default)]
pub struct RuntimeNode {
    pub node_id: u32,
    index: usize,

    pub binding_behavior: BindingBehavior,
    pub should_execute: bool,
    pub has_missing_inputs: bool,
    pub executed: bool,
    pub has_outputs: bool,
    pub inputs: Args,
    pub outputs: Args,
    pub run_time: f64,
    pub execution_index: u32,
}

pub struct Runtime {
    nodes: Vec<RuntimeNode>,
    order: Vec<usize>,
}

impl Runtime {
    pub fn new() -> Runtime {
        Runtime {
            nodes: Vec::new(),
            order: Vec::new(),
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

    pub fn run(&mut self, graph: &Graph, invoker: &dyn Invoker) {
        self.prepare(graph);

        invoker.start();
        let mut execution_index: u32 = 0;

        for i in 0..self.order.len() {
            let index = self.order[i];
            let mut rnode = mem::replace(&mut self.nodes[index], RuntimeNode::default());
            let node = graph.node_by_id(rnode.node_id()).unwrap();

            assert!(rnode.should_execute);

            for (input_index, input) in node.inputs.iter().enumerate() {
                let binding = input.binding.as_ref().unwrap();
                assert_ne!(binding.node_id(), node.id());

                let output_runtime_node = self.node_by_id(binding.node_id()).unwrap();

                assert_eq!(output_runtime_node.has_outputs, true);

                if output_runtime_node.executed {
                    if is_debug() {
                        let output_node = graph.node_by_id(binding.node_id()).unwrap();
                        let output_arg = output_node.outputs.get(binding.output_index()).unwrap();
                        let output_value = &output_runtime_node.outputs[binding.output_index()];
                        assert_eq!(input.data_type, output_arg.data_type);
                        assert_eq!(input.data_type, output_value.data_type());
                    }

                    rnode.inputs[input_index] = output_runtime_node.outputs[binding.output_index()].clone();
                }
            }

            let start = Instant::now();
            invoker.call(&node.name, rnode.node_id(), &rnode.inputs, &mut rnode.outputs);
            rnode.run_time = start.elapsed().as_secs_f64();
            rnode.has_outputs = true;
            rnode.execution_index = execution_index;
            rnode.executed = true;
            execution_index += 1;

            self.nodes[index] = rnode;
        }

        invoker.finish();
    }

    fn prepare(&mut self, graph: &Graph) {
        assert!(graph.validate());

        let mut last_run = mem::replace(&mut self.nodes, Vec::new());

        self.nodes = graph.nodes().iter().enumerate().map(
            |(i, node)| -> RuntimeNode{
                RuntimeNode::new(i, node, &mut last_run)
            }
        ).collect();

        self.traverse_backward(graph);
        self.traverse_forward(graph);
    }
    fn traverse_backward(&mut self, graph: &Graph) {
        let active_nodes = graph.nodes().iter().filter(|node| node.is_output);
        self.order.clear();

        for node in active_nodes {
            let mut rnode = self.node_by_id_mut(node.id());
            rnode.has_outputs = true;
            rnode.binding_behavior = BindingBehavior::Always;
            rnode.should_execute = true;

            let index = rnode.index;
            self.order.push(index);
        }

        let mut i: usize = 0;
        while i < self.order.len() {
            let index = self.order[i];
            let node_id: u32 = self.nodes[index].node_id();
            let node_binding_behavior: BindingBehavior = self.nodes[index].binding_behavior;
            let node = graph.node_by_id(node_id).unwrap();

            for input in node.inputs.iter() {
                if input.binding.is_none() {
                    continue;
                }

                let binding = input.binding.as_ref().unwrap();
                assert_ne!(binding.node_id(), node.id());
                let output_node = graph.node_by_id(binding.node_id()).unwrap();
                let output_rnode: &mut RuntimeNode = self.node_by_id_mut(output_node.id());

                assert_eq!(output_rnode.inputs.len(), output_node.inputs.len());
                assert_eq!(output_rnode.outputs.len(), output_node.outputs.len());

                if node_binding_behavior == BindingBehavior::Always
                    && binding.behavior == BindingBehavior::Always {
                    output_rnode.binding_behavior = BindingBehavior::Always;
                }

                let output_index = output_rnode.index;
                self.order.push(output_index);
            }

            i += 1;
        }


        self.order.reverse();

        // dedup preserving first occurrence
        let mut set: HashSet<usize> = HashSet::new();
        self.order.retain(|e| set.insert(*e));
    }
    fn traverse_forward(&mut self, graph: &Graph) {
        let mut new_order: Vec<usize> = Vec::new();

        for i in 0..self.order.len() {
            let index = self.order[i];
            let rnode = &self.nodes[index];
            let node = graph.node_by_id(rnode.node_id()).unwrap();

            let mut has_missing_inputs: bool = false;
            let mut should_execute: bool = rnode.should_execute;

            for input in node.inputs.iter() {
                match input.binding.as_ref() {
                    None => {
                        if input.is_required {
                            has_missing_inputs = true;
                        }
                    }
                    Some(binding) => {
                        assert_ne!(binding.node_id(), node.id());
                        let output_rnode = self.nodes.iter().find(|_node| _node.node_id() == binding.node_id()).unwrap();
                        assert_eq!(output_rnode.has_outputs || output_rnode.should_execute, true);
                        if output_rnode.has_missing_inputs {
                            has_missing_inputs = true;
                        }
                        if binding.behavior == BindingBehavior::Always && output_rnode.should_execute {
                            should_execute = true;
                        }
                    }
                }
            }

            if has_missing_inputs {
                should_execute = false;
            } else if node.is_output {
                should_execute = true;
            } else if !rnode.has_outputs {
                should_execute = true;
            } else if rnode.binding_behavior == BindingBehavior::Once {
                should_execute = false;
            } else if node.behavior == NodeBehavior::Active {
                should_execute = true;
            }

            self.nodes[index].should_execute = should_execute;

            if should_execute {
                new_order.push(index);
            }
        }

        self.order = new_order;
    }
}

impl RuntimeNode {
    pub fn node_id(&self) -> u32 {
        self.node_id
    }


    pub fn new(index: usize, node: &Node, last_run: &mut Vec<RuntimeNode>) -> RuntimeNode {
        let inputs;
        let mut outputs;
        let mut has_outputs = false;

        let existing_rnode
            = last_run.iter_mut().find(|_last_run_node| _last_run_node.node_id() == node.id());
        if let Some(existing_rnode) = existing_rnode {
            has_outputs = existing_rnode.has_outputs;
            inputs = mem::replace(&mut existing_rnode.inputs, Args::new());
            outputs = mem::replace(&mut existing_rnode.outputs, Args::new());
        } else {
            inputs = Args::with_size(node.inputs.len());
            outputs = Args::with_size(node.outputs.len());

            for (i, output) in node.outputs.iter().enumerate() {
                outputs[i] = output.data_type.into();
            }
        }

        let result = RuntimeNode {
            index,
            node_id: node.id(),
            has_missing_inputs: false,
            binding_behavior: BindingBehavior::Once,
            should_execute: false,
            has_outputs,
            inputs,
            outputs,
            run_time: 0.0,
            execution_index: 0,
            executed: false,
        };

        return result;
    }
}