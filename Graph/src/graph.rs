use serde::{Serialize, Deserialize};
use crate::data_type::DataType;
use bevy_ecs::prelude::Component;


#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeBehavior {
    Active,
    Passive,
}


#[derive(Clone, Component, Serialize, Deserialize)]
pub struct Node {
    self_id: u32,

    pub name: String,
    pub behavior: NodeBehavior,
    pub is_output: bool,

    pub inputs: Vec<Input>,
    pub outputs: Vec<Output>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Output {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BindingBehavior {
    Always,
    Once,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Binding {
    node_id: u32,
    output_index: usize,
    pub behavior: BindingBehavior,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
    pub data_type: DataType,
    pub is_required: bool,
    pub binding: Option<Binding>,
}


#[derive(Serialize, Deserialize)]
pub struct Graph {
    new_id: u32,

    nodes: Vec<Node>,
}


impl Graph {
    pub fn new() -> Graph {
        Graph {
            new_id: 5000,
            nodes: Vec::new(),
        }
    }

    fn new_id(&mut self) -> u32 {
        let id = self.new_id;
        self.new_id += 1;
        return id;
    }

    pub fn nodes(&self) -> &Vec<Node> {
        &self.nodes
    }
    pub fn nodes_mut(&mut self) -> Vec<&mut Node> {
        self.nodes.iter_mut().collect()
    }

    pub fn add_node(&mut self, node: &mut Node) {
        if let Some(existing_node) = self.node_by_id_mut(node.id()) {
            *existing_node = node.clone();
        } else {
            node.self_id = self.new_id();
            self.nodes.push(node.clone());
        }
    }

    pub fn remove_node_by_id(&mut self, id: u32) {
        assert_ne!(id, 0);

        self.nodes.retain(|node| node.self_id != id);

        self.nodes.iter_mut().flat_map(|node| node.inputs.iter_mut())
            .filter(|input| input.binding.is_some() && input.binding.as_ref().unwrap().node_id == id)
            .for_each(|input| {
                input.binding = None;
            });
    }


    pub fn node_by_id(&self, id: u32) -> Option<&Node> {
        if id == 0 {
            return None;
        }
        self.nodes.iter().find(|node| node.self_id == id)
    }

    pub fn node_by_id_mut(&mut self, id: u32) -> Option<&mut Node> {
        if id == 0 {
            return None;
        }
        self.nodes.iter_mut().find(|node| node.self_id == id)
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

        for node in self.nodes.iter() {
            if node.self_id == 0 {
                return false;
            }

            for input in node.inputs.iter() {
                if let Some(binding) = &input.binding {
                    if self.node_by_id(binding.node_id).is_none() {
                        return false;
                    }
                }
            }
        }

        return true;
    }
}


impl Node {
    pub fn new() -> Node {
        Node {
            self_id: 0,
            name: String::new(),
            behavior: NodeBehavior::Active,
            is_output: false,
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub fn id(&self) -> u32 {
        self.self_id
    }
}

impl Input {
    pub fn new() -> Input {
        Input {
            binding: None,
            name: String::new(),
            data_type: DataType::None,
            is_required: false,
        }
    }
}

impl Output {
    pub fn new() -> Output {
        Output {
            name: String::new(),
            data_type: DataType::None,
        }
    }
}

impl Binding {
    pub fn node_id(&self) -> u32 {
        self.node_id
    }
    pub fn output_index(&self) -> usize {
        self.output_index
    }

    pub fn new(node_id: u32, output_index: usize) -> Binding {
        Binding {
            node_id,
            output_index,
            behavior: BindingBehavior::Always,
        }
    }
}