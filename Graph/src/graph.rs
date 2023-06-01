use serde::{Serialize, Deserialize};
use uuid::Uuid;
use crate::data_type::DataType;


#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum NodeBehavior {
    Active,
    Passive,
}


#[derive(Clone, Serialize, Deserialize)]
pub struct Node {
    self_id: Uuid,

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

#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BindingBehavior {
    #[default]
    Always,
    Once,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Binding {
    output_node_id: Uuid,
    output_index: u32,
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
    nodes: Vec<Node>,
}


impl Graph {
    pub fn new() -> Graph {
        Graph {
            nodes: Vec::new(),
        }
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
            self.nodes.push(node.clone());
        }
    }

    pub fn remove_node_by_name(&mut self, name: &str) -> Result<(), ()> {
        let node = self.nodes.iter().find(|node| node.name == name).ok_or(())?;
        self.remove_node_by_id(node.self_id);

        Ok(())
    }
    pub fn remove_node_by_id(&mut self, id: Uuid) {
        assert_ne!(id, Uuid::nil());

        self.nodes.retain(|node| node.self_id != id);

        self.nodes.iter_mut().flat_map(|node| node.inputs.iter_mut())
            .filter(|input| input.binding.is_some() && input.binding.as_ref().unwrap().output_node_id == id)
            .for_each(|input| {
                input.binding = None;
            });
    }


    pub fn node_by_name(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.name == name)
    }
    pub fn node_by_name_mut(&mut self, name: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|node| node.name == name)
    }

    pub fn node_by_id(&self, id: Uuid) -> Option<&Node> {
        if id == Uuid::nil() {
            return None;
        }
        self.nodes.iter().find(|node| node.self_id == id)
    }

    pub fn node_by_id_mut(&mut self, id: Uuid) -> Option<&mut Node> {
        if id == Uuid::nil() {
            return None;
        }
        self.nodes.iter_mut().find(|node| node.self_id == id)
    }


    pub fn to_yaml(&self) -> String { serde_yaml::to_string(&self).unwrap() }
    pub fn from_yaml_file(path: &str) -> Graph {
        let yaml = std::fs::read_to_string(path).unwrap();
        let graph: Graph = serde_yaml::from_str(&yaml).unwrap();

        if !graph.validate() {
            panic!("Invalid graph");
        }

        return graph;
    }
    pub fn from_yaml(yaml: &str) -> Graph {
        let graph: Graph = serde_yaml::from_str(&yaml).unwrap();

        if !graph.validate() {
            panic!("Invalid graph");
        }

        return graph;
    }

    pub fn validate(&self) -> bool {
        if self.nodes.iter().any(|node| node.self_id == Uuid::nil()) {
            return false;
        }

        for node in self.nodes.iter() {
            if node.self_id == Uuid::nil() {
                return false;
            }

            for input in node.inputs.iter() {
                if let Some(binding) = &input.binding {
                    if self.node_by_id(binding.output_node_id).is_none() {
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
            self_id: Uuid::new_v4(),
            name: String::new(),
            behavior: NodeBehavior::Active,
            is_output: false,
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub fn id(&self) -> Uuid {
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
    pub fn output_node_id(&self) -> Uuid {
        self.output_node_id
    }
    pub fn output_index(&self) -> u32 {
        self.output_index
    }

    pub fn new(node_id: Uuid, output_index: u32) -> Binding {
        Binding {
            output_node_id: node_id,
            output_index,
            behavior: BindingBehavior::Always,
        }
    }
}