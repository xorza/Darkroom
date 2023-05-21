use crate::node::*;
use crate::function::*;
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


    pub fn node_by_id(&self, id: u32) -> Option<&Node> {
        self.nodes.iter().find(|node| node.self_id == id)
    }
    pub fn node_by_id_mut(&mut self, id: u32) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|node| node.self_id == id)
    }

    pub fn output_by_id(&self, id: u32) -> Option<&Output> {
        self.outputs.iter().find(|output| output.self_id == id)
    }
    pub fn input_by_id(&self, id: u32) -> Option<&Input> {
        self.inputs.iter().find(|input| input.self_id == id)
    }

    pub fn inputs_by_node_id(&self, node_id: u32) -> Vec<&Input> {
        self.inputs.iter().filter(|input| input.node_id == node_id).collect()
    }
    pub fn outputs_by_node_id(&self, node_id: u32) -> Vec<&Output> {
        self.outputs.iter().filter(|output| output.node_id == node_id).collect()
    }
    pub fn edge_by_input_id(&self, input_id: u32) -> Option<&Edge> {
        self.edges.iter().find(|edge| edge.input_id == input_id)
    }
    pub fn edge_by_input_id_mut(&mut self, input_id: u32) -> Option<&mut Edge> {
        self.edges.iter_mut().find(|edge| edge.input_id == input_id)
    }
    pub fn output_for_input_id(&self, input_id: u32) -> Option<&Output> {
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

        return graph;
    }
}