use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::data_type::DataType;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum NodeBehavior {
    #[default]
    Active,
    Passive,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Node {
    self_id: Uuid,

    pub function_id: Uuid,

    pub name: String,
    pub behavior: NodeBehavior,
    pub is_output: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<Output>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subgraph_id: Option<Uuid>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Output {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<Binding>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SubArgument {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SubArgumentConnection {
    pub subargument_index: u32,
    pub subnode_id: Uuid,
    pub subnode_argument_index: u32,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SubGraph {
    self_id: Uuid,

    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<SubArgument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_subnode_connections: Vec<SubArgumentConnection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<SubArgument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_subnode_connections: Vec<SubArgumentConnection>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Graph {
    nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subgraphs: Vec<SubGraph>,
}


impl Graph {
    pub fn nodes(&self) -> &Vec<Node> {
        &self.nodes
    }
    pub fn nodes_mut(&mut self) -> Vec<&mut Node> {
        self.nodes.iter_mut().collect()
    }

    pub fn add_node(&mut self, node: Node) {
        match self.nodes.iter().position(|n| n.self_id == node.self_id) {
            Some(index) => self.nodes[index] = node,
            None => self.nodes.push(node),
        }
    }
    pub fn remove_node_by_id(&mut self, id: Uuid) {
        assert_ne!(id, Uuid::nil());

        self.nodes.retain(|node| node.self_id != id);

        self.nodes
            .iter_mut()
            .flat_map(|node| node.inputs.iter_mut())
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
        assert_ne!(id, Uuid::nil());

        self.nodes
            .iter()
            .find(|node| node.self_id == id)
    }
    pub fn node_by_id_mut(&mut self, id: Uuid) -> Option<&mut Node> {
        assert_ne!(id, Uuid::nil());

        self.nodes
            .iter_mut()
            .find(|node| node.self_id == id)
    }
    pub fn nodes_by_subgraph_id(&self, subgraph_id: Uuid) -> Vec<&Node> {
        assert_ne!(subgraph_id, Uuid::nil());

        self.nodes
            .iter()
            .filter(|node| node.subgraph_id == Some(subgraph_id))
            .collect()
    }

    pub fn to_yaml(&self) -> anyhow::Result<String> {
        let yaml = serde_yaml::to_string(&self)?;
        Ok(yaml)
    }
    pub fn from_yaml_file(path: &str) -> anyhow::Result<Graph> {
        let yaml = std::fs::read_to_string(path)?;
        let graph: Graph = serde_yaml::from_str(&yaml)?;

        graph.validate()?;

        Ok(graph)
    }
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Graph> {
        let graph: Graph = serde_yaml::from_str(yaml)?;

        graph.validate()?;

        Ok(graph)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        for node in self.nodes.iter() {
            if let Some(subgraph_id) = node.subgraph_id {
                self.subgraph_by_id(subgraph_id).ok_or(anyhow::Error::msg("Node has invalid subgraph id"))?;
            }

            if node.self_id == Uuid::nil() {
                return Err(anyhow::Error::msg("Node has invalid id"));
            }

            for input in node.inputs.iter() {
                if let Some(binding) = &input.binding {
                    if self.node_by_id(binding.output_node_id).is_none() {
                        return Err(anyhow::Error::msg("Node has invalid binding"));
                    }
                }
            }
        }

        for subgraph in self.subgraphs.iter() {
            for input in subgraph.inputs.iter() {
                // todo!("validate subgraph inputs")
                // let node = self
                //     .node_by_id(input.node_id)
                //     .ok_or(anyhow::Error::msg("Subgraph has invalid input"))?;
                // if node.subgraph_id != Some(subgraph.self_id) {
                //     return Err(anyhow::Error::msg("Subgraph has invalid input"));
                // }
            }
        }

        Ok(())
    }

    pub fn subgraphs(&self) -> &Vec<SubGraph> {
        &self.subgraphs
    }

    pub fn add_subgraph(&mut self, subgraph: &SubGraph) {
        match self.subgraphs.iter().position(|sg| sg.self_id == subgraph.self_id) {
            Some(index) => self.subgraphs[index] = subgraph.clone(),
            None => self.subgraphs.push(subgraph.clone()),
        }
    }
    pub fn remove_subgraph_by_id(&mut self, id: Uuid) {
        assert_ne!(id, Uuid::nil());

        self.subgraphs
            .retain(|subgraph| subgraph.self_id != id);

        self.nodes
            .iter()
            .filter(|node| node.subgraph_id == Some(id))
            .map(|node| node.self_id)
            .collect::<Vec<Uuid>>()
            .iter()
            .cloned()
            .for_each(|node_id| {
                self.remove_node_by_id(node_id);
            });
    }

    pub fn subgraph_by_id_mut(&mut self, id: Uuid) -> Option<&mut SubGraph> {
        assert_ne!(id, Uuid::nil());
        self.subgraphs
            .iter_mut()
            .find(|subgraph| subgraph.self_id == id)
    }
    pub fn subgraph_by_id(&self, id: Uuid) -> Option<&SubGraph> {
        assert_ne!(id, Uuid::nil());
        self.subgraphs
            .iter()
            .find(|subgraph| subgraph.self_id == id)
    }
}

impl Node {
    pub fn new() -> Node {
        Node {
            self_id: Uuid::new_v4(),
            function_id: Uuid::nil(),
            name: String::new(),
            behavior: NodeBehavior::Active,
            is_output: false,
            inputs: Vec::new(),
            outputs: Vec::new(),
            subgraph_id: None,
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

impl SubGraph {
    pub fn new() -> SubGraph {
        SubGraph {
            self_id: Uuid::new_v4(),
            name: String::new(),
            inputs: Vec::new(),
            input_subnode_connections: Vec::new(),
            outputs: Vec::new(),
            output_subnode_connections: Vec::new(),
        }
    }

    pub fn id(&self) -> Uuid {
        self.self_id
    }
}
