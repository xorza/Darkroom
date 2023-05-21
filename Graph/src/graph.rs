use crate::node::*;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct Graph {
    new_id: u32,

    pub nodes: Vec<Node>,
    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
    pub edges: Vec<Edge>,
}


impl Graph {
    pub fn new() -> Graph {
        Graph {
            new_id: 5000,
            nodes: Vec::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            edges: Vec::new(),
        }
    }

    fn new_id(&mut self) -> u32 {
        let id = self.new_id;
        self.new_id += 1;
        return id;
    }


    pub fn add_node(&mut self, node: &mut Node) {
        node.self_id = self.new_id();
        self.nodes.push(node.clone());
    }
    pub fn add_input(&mut self, input: &mut Input) {
        input.self_id = self.new_id();
        self.inputs.push(input.clone());
    }
    pub fn add_output(&mut self, output: &mut Output) {
        output.self_id = self.new_id();
        self.outputs.push(output.clone());
    }
    pub fn add_edge(&mut self, edge: &Edge) {
        self.edges.retain(|_edge| _edge.input_id != edge.input_id);
        self.edges.push(edge.clone());
    }

    pub fn node_remove_by_id(&mut self, id: u32) {
        let input_ids = self.inputs_by_node_id(id)
            .map(|input| input.self_id)
            .collect::<Vec<u32>>();
        let output_ids = self.outputs_by_node_id(id)
            .map(|output| output.self_id)
            .collect::<Vec<u32>>();

        self.edges.retain(|edge| {
            !input_ids.contains(&edge.input_id)
                && !output_ids.contains(&edge.output_id)
        });
        self.inputs.retain(|input| input.node_id != id);
        self.outputs.retain(|output| output.node_id != id);
        self.nodes.retain(|node| node.self_id != id);
    }


    pub fn node_by_id(&self, id: u32) -> Option<&Node> {
        assert_ne!(id, 0);
        self.nodes.iter().find(|node| node.self_id == id)
    }
    pub fn node_by_id_mut(&mut self, id: u32) -> Option<&mut Node> {
        assert_ne!(id, 0);
        self.nodes.iter_mut().find(|node| node.self_id == id)
    }

    pub fn output_by_id(&self, id: u32) -> Option<&Output> {
        assert_ne!(id, 0);
        self.outputs.iter().find(|output| output.self_id == id)
    }
    pub fn input_by_id(&self, id: u32) -> Option<&Input> {
        assert_ne!(id, 0);
        self.inputs.iter().find(|input| input.self_id == id)
    }

    pub fn inputs_by_node_id(&self, node_id: u32) -> impl Iterator<Item=&Input> {
        assert_ne!(node_id, 0);
        self.inputs.iter().filter(move |input| input.node_id == node_id)
    }
    pub fn outputs_by_node_id(&self, node_id: u32) -> impl Iterator<Item=&Output> {
        assert_ne!(node_id, 0);
        self.outputs.iter().filter(move |output| output.node_id == node_id)
    }
    pub fn edge_by_input_id(&self, input_id: u32) -> Option<&Edge> {
        assert_ne!(input_id, 0);
        self.edges.iter().find(|edge| edge.input_id == input_id)
    }
    pub fn edge_by_input_id_mut(&mut self, input_id: u32) -> Option<&mut Edge> {
        assert_ne!(input_id, 0);
        self.edges.iter_mut().find(|edge| edge.input_id == input_id)
    }
    pub fn output_for_input_id(&self, input_id: u32) -> Option<&Output> {
        assert_ne!(input_id, 0);
        let edge = self.edge_by_input_id(input_id);
        match edge {
            Some(edge) => self.outputs.iter().find(|output| output.self_id == edge.output_id),
            None => None,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }
    pub fn from_json_file(path: &str) -> Graph {
        let json = std::fs::read_to_string(path).unwrap();
        let graph: Graph = serde_json::from_str(&json).unwrap();

        if !graph.validate() {
            panic!("Invalid graph");
        }

        return graph;
    }

    pub fn validate(&self) -> bool {
        if self.nodes.iter().any(|node| node.self_id == 0) {
            return false;
        }

        if self.inputs.iter().any(|input| input.self_id == 0 || self.node_by_id(input.node_id).is_none()) {
            return false;
        }
        if self.outputs.iter().any(|output| output.self_id == 0 || self.node_by_id(output.node_id).is_none()) {
            return false;
        }

        for edge in &self.edges {
            if edge.input_id == 0 || edge.output_id == 0 {
                return false;
            }

            let input = self.input_by_id(edge.input_id);
            let output = self.output_by_id(edge.output_id);
            if input.is_none() || output.is_none() {
                return false;
            }
            if input.unwrap().node_id == output.unwrap().node_id {
                return false;
            }
            if input.unwrap().data_type != output.unwrap().data_type {
                return false;
            }
        }

        return true;
    }
}