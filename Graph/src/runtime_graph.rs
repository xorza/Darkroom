use crate::graph::*;

#[derive(Clone)]
pub struct RuntimeNode {
    node_id: u32,

    pub has_missing_inputs: bool,
    pub binding_behavior: BindingBehavior,
    pub should_execute: bool,
    pub has_outputs: bool,
}

pub struct RuntimeGraph {
    nodes: Vec<RuntimeNode>,
}

impl RuntimeGraph {
    pub fn new() -> RuntimeGraph {
        RuntimeGraph {
            nodes: Vec::new(),
        }
    }

    pub fn nodes(&self) -> &Vec<RuntimeNode> {
        &self.nodes
    }

    pub fn run(&mut self, graph: &Graph) {
        assert!(graph.validate());

        let last_run = self.nodes.clone();
        self.nodes.clear();

        self.traverse_backward(graph, last_run);
        self.traverse_forward(graph);
    }

    pub fn node_by_id(&self, node_id: u32) -> Option<&RuntimeNode> {
        self.nodes.iter().find(|node| node.node_id() == node_id)
    }

    fn traverse_backward(&mut self, graph: &Graph, last_run: Vec<RuntimeNode>) {
        let active_nodes = graph.nodes().iter().filter(|node| node.is_output);
        for node in active_nodes {
            let mut runtime_node = RuntimeNode {
                node_id: node.id(),
                has_missing_inputs: false,
                binding_behavior: BindingBehavior::Always,
                should_execute: true,
                has_outputs: true,
            };
            runtime_node.has_outputs = true;
            self.nodes.push(runtime_node);
        }

        let mut i: usize = 0;
        while i < self.nodes.len() {
            let mut i_node = self.nodes[i].clone();
            let node = graph.node_by_id(i_node.node_id()).unwrap();

            for input in &node.inputs {
                match &input.binding {
                    None => { i_node.has_missing_inputs = true; }
                    Some(binding) => {
                        let output_node = graph.node_by_id(binding.node_id).unwrap();

                        let mut output_i_node: &mut RuntimeNode;
                        if let Some(_node) = self.nodes.iter_mut().find(|node| node.node_id() == output_node.id()) {
                            output_i_node = _node;
                        } else {
                            self.nodes.push(RuntimeNode {
                                node_id: output_node.id(),
                                has_missing_inputs: false,
                                binding_behavior: BindingBehavior::Once,
                                should_execute: true,
                                has_outputs: false,
                            });
                            output_i_node = self.nodes.last_mut().unwrap();
                            if let Some(last_run_node) = last_run.iter().find(|_last_run_node| _last_run_node.node_id() == output_node.id()) {
                                output_i_node.has_outputs = last_run_node.has_outputs || last_run_node.should_execute;
                            }
                        }

                        if i_node.binding_behavior == BindingBehavior::Always
                            && binding.behavior == BindingBehavior::Always {
                            output_i_node.binding_behavior = BindingBehavior::Always;
                        }
                    }
                }
            }

            self.nodes[i] = i_node;
            i += 1;
        }

        self.nodes.reverse();
    }


    fn traverse_forward(&mut self, graph: &Graph) {
        let mut i: usize = 0;
        while i < self.nodes.len() {
            let mut i_node = self.nodes[i].clone();
            let node = graph.node_by_id(i_node.node_id()).unwrap();

            for input in node.inputs.iter() {
                match input.binding.as_ref() {
                    None => {
                        if input.is_required {
                            i_node.has_missing_inputs = true;
                        }
                    }
                    Some(binding) => {
                        let output_i_node = self.nodes.iter().find(|_node| _node.node_id() == binding.node_id).unwrap();
                        assert_eq!(output_i_node.has_outputs || output_i_node.should_execute, true);
                        if output_i_node.has_missing_inputs {
                            i_node.has_missing_inputs = true;
                        }
                    }
                }
            }

            i_node.should_execute = self.should_execute(node, &i_node);
            self.nodes[i] = i_node;

            i += 1;
        }
    }

    fn should_execute(&self, node: &Node, i_node: &RuntimeNode) -> bool {
        if node.is_output {
            return true;
        }

        if i_node.has_missing_inputs {
            return false;
        }

        if !i_node.has_outputs {
            return true;
        }

        if i_node.binding_behavior == BindingBehavior::Once {
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
                        let output_i_node = self.nodes.iter().find(|_node| _node.node_id() == binding.node_id).unwrap();
                        if output_i_node.should_execute {
                            return true;
                        }
                    }
                }
            }
        }

        return false;
    }
}

impl RuntimeNode {
    pub fn node_id(&self) -> u32 {
        self.node_id
    }
}