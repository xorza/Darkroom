use crate::graph::*;

#[derive(Clone)]
pub struct RuntimeNode {
    node_id: u32,

    pub behavior: NodeBehavior,
    pub is_complete: bool,
    pub edge_behavior: ConnectionBehavior,
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
        self.traverse_forward1(graph);
        self.traverse_forward2(graph);
    }

    pub fn node_by_id(&self, node_id: u32) -> Option<&RuntimeNode> {
        self.nodes.iter().find(|node| node.node_id() == node_id)
    }

    fn traverse_backward(&mut self, graph: &Graph, last_run: Vec<RuntimeNode>) {
        let active_nodes: Vec<&Node> = graph.nodes().iter().filter(|node| node.is_output).collect();
        for node in active_nodes {
            self.nodes.push(RuntimeNode::new(node.id()));
        }

        let mut i: usize = 0;
        while i < self.nodes.len() {
            let mut i_node = self.nodes[i].clone();

            let inputs = graph.inputs_by_node_id(i_node.node_id());
            for input in inputs {
                if input.connected_output_id == 0 {
                    i_node.is_complete = false;
                } else {
                    let output = graph.output_by_id(input.connected_output_id).unwrap();
                    let output_node = graph.node_by_id(output.node_id()).unwrap();

                    let mut output_i_node: &mut RuntimeNode;
                    if let Some(_node) = self.nodes.iter_mut().find(|node| node.node_id() == output_node.id()) {
                        output_i_node = _node;
                    } else {
                        self.nodes.push(RuntimeNode::new(output_node.id()));
                        output_i_node = self.nodes.last_mut().unwrap();
                        output_i_node.behavior = output_node.behavior;
                        output_i_node.edge_behavior = ConnectionBehavior::Once;

                        if let Some(_node) = last_run.iter().find(|node| node.node_id() == output_node.id()) {
                            output_i_node.has_outputs = _node.has_outputs;
                        }
                    }

                    if i_node.edge_behavior == ConnectionBehavior::Always
                        && input.connection_behavior == ConnectionBehavior::Always {
                        output_i_node.edge_behavior = ConnectionBehavior::Always;
                    }
                }
            }

            self.nodes[i] = i_node;
            i += 1;
        }

        self.nodes.reverse();
    }

    fn traverse_forward1(&mut self, graph: &Graph) {
        for i in 0..self.nodes.len() {
            let mut i_node = self.nodes[i].clone();

            let inputs = graph.inputs_by_node_id(i_node.node_id());
            for input in inputs {
                if input.connected_output_id == 0 {
                    if input.is_required {
                        i_node.is_complete = false;
                    }
                } else {
                    let output = graph.output_by_id(input.connected_output_id).unwrap();
                    let output_i_node = self.nodes.iter().find(|_node| _node.node_id() == output.node_id()).unwrap();
                    if output_i_node.is_complete == false {
                        i_node.is_complete = false;
                    }
                }
            }

            self.nodes[i] = i_node;
        }
    }

    fn traverse_forward2(&mut self, graph: &Graph) {
        for i in 0..self.nodes.len() {
            let mut i_node = self.nodes[i].clone();

            if self.can_skip(graph, &i_node) {
                continue;
            }

            i_node.should_execute = true;
            i_node.has_outputs = true;
            self.nodes[i] = i_node;
        }
    }

    fn can_skip(&mut self, graph: &Graph, i_node: &RuntimeNode) -> bool {
        if i_node.is_complete == false {
            return true;
        }

        if i_node.has_outputs == false {
            return false;
        }

        if i_node.edge_behavior == ConnectionBehavior::Once {
            return true;
        }

        if i_node.behavior == NodeBehavior::Passive
            && !self.has_updated_inputs(graph, i_node.node_id()) {
            return true;
        }


        return false;
    }

    fn has_updated_inputs(&mut self, graph: &Graph, node_id: u32) -> bool {
        let mut has_updated_inputs = false;

        for input in graph.inputs_by_node_id(node_id) {
            if input.connected_output_id == 0 {
                debug_assert_eq!(input.is_required, false);
            } else if input.connection_behavior == ConnectionBehavior::Always {
                let output = graph.output_by_id(input.connected_output_id).unwrap();
                let output_i_node =
                    self.nodes.iter_mut()
                        .find(|_i_node| _i_node.node_id() == output.node_id())
                        .unwrap();

                if output_i_node.should_execute {
                    has_updated_inputs = true;
                }
            }
        }
        return has_updated_inputs;
    }
}

impl RuntimeNode {
    pub fn new(node_id: u32) -> RuntimeNode {
        RuntimeNode {
            node_id,
            behavior: NodeBehavior::Active,
            edge_behavior: ConnectionBehavior::Always,
            is_complete: true,
            should_execute: false,
            has_outputs: false,
        }
    }

    pub fn node_id(&self) -> u32 {
        self.node_id
    }
}