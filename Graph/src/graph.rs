use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::data::{DataType, Value};
use crate::functions::Function;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum FunctionBehavior {
    #[default]
    Active,
    Passive,
}


#[derive(Clone, Serialize, Deserialize)]
pub struct Node {
    self_id: Uuid,

    pub function_id: Uuid,

    pub name: String,
    pub behavior: FunctionBehavior,
    pub is_output: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<Output>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subgraph_id: Option<Uuid>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
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

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputBinding {
    pub output_node_id: Uuid,
    pub output_index: u32,
    pub behavior: BindingBehavior,
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub enum Binding {
    #[default]
    None,
    Const,
    Output(OutputBinding),
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Input {
    pub name: String,
    pub data_type: DataType,
    pub is_required: bool,
    pub binding: Binding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub const_value: Option<Value>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SubInputNodeConnection {
    pub subnode_id: Uuid,
    pub subnode_input_index: u32,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SubInput {
    pub name: String,
    pub data_type: DataType,
    pub is_required: bool,
    pub connections: Vec<SubInputNodeConnection>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SubOutput {
    pub name: String,
    pub data_type: DataType,
    pub subnode_id: Uuid,
    pub subnode_output_index: u32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SubGraph {
    self_id: Uuid,

    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<SubInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<SubOutput>,
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
            .filter_map(|input| match &input.binding {
                Binding::Output(output_binding) if output_binding.output_node_id == id => Some(input),
                _ => None,
            })
            .for_each(|input| {
                input.binding = input.const_value.as_ref()
                    .map_or(Binding::None, |_| Binding::Const);
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
            if node.self_id == Uuid::nil() {
                return Err(anyhow::Error::msg("Node has invalid id"));
            }

            // validate node has a valid subgraph
            if let Some(subgraph_id) = node.subgraph_id {
                self.subgraph_by_id(subgraph_id).ok_or(anyhow::Error::msg("Node has invalid subgraph id"))?;
            }

            // validate node has valid bindings
            for input in node.inputs.iter() {
                if let Binding::Output(output_binding) = &input.binding {
                    if self.node_by_id(output_binding.output_node_id).is_none() {
                        return Err(anyhow::Error::msg("Node input connected to a non-existent node"));
                    }
                }
            }
        }

        for subgraph in self.subgraphs.iter() {
            // validate all subgraph inputs are connected
            for subinput in subgraph.inputs.iter() {
                for connection in subinput.connections.iter() {
                    let node = self.node_by_id(connection.subnode_id)
                        .ok_or(anyhow::Error::msg("Subgraph input connected to a non-existent node"))?;
                    if node.subgraph_id != Some(subgraph.self_id) {
                        return Err(anyhow::Error::msg("Subgraph input connected to an external node"));
                    }
                    let input = node.inputs.get(connection.subnode_input_index as usize)
                        .ok_or(anyhow::Error::msg("Subgraph input connected to a non-existent input"))?;

                    if !DataType::can_assign(&subinput.data_type, &input.data_type) {
                        return Err(anyhow::Error::msg("Subgraph input connected to a node input with an incompatible data type"));
                    }
                }
            }

            for suboutput in subgraph.outputs.iter() {
                let node = self.node_by_id(suboutput.subnode_id)
                    .ok_or(anyhow::Error::msg("Subgraph output connected to a non-existent node"))?;
                if node.subgraph_id != Some(subgraph.self_id) {
                    return Err(anyhow::Error::msg("Subgraph output connected to an external node"));
                }

                let output = node.outputs.get(suboutput.subnode_output_index as usize)
                    .ok_or(anyhow::Error::msg("Subgraph output connected to a non-existent output"))?;
                if !DataType::can_assign(&suboutput.data_type, &output.data_type) {
                    return Err(anyhow::Error::msg("Subgraph output connected to a node output with an incompatible data type"));
                }
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
            name: "".to_string(),
            behavior: FunctionBehavior::Active,
            is_output: false,
            inputs: vec![],
            outputs: vec![],
            subgraph_id: None,
        }
    }

    pub fn from_function(function: &Function) -> Node {
        let inputs: Vec<Input> = function.inputs.iter().map(|func_input| {
            Input {
                name: func_input.name.clone(),
                data_type: func_input.data_type.clone(),
                is_required: true,
                binding: func_input.const_value.as_ref().map_or(Binding::None, |_| Binding::Const),
                const_value: func_input.const_value.clone(),
            }
        }).collect();

        let outputs: Vec<Output> = function.outputs.iter().map(|output| {
            Output {
                name: output.name.clone(),
                data_type: output.data_type.clone(),
            }
        }).collect();

        Node {
            self_id: Uuid::new_v4(),
            function_id: function.id(),
            name: function.name.clone(),
            behavior: FunctionBehavior::Active,
            is_output: false,
            inputs,
            outputs,
            subgraph_id: None,
        }
    }

    pub fn id(&self) -> Uuid {
        self.self_id
    }
}

impl Binding {
    pub fn from_output_binding(output_node_id: Uuid, output_index: u32) -> Binding {
        Binding::Output(OutputBinding {
            output_node_id,
            output_index,
            behavior: BindingBehavior::default(),
        })
    }

    pub fn as_output_binding(&self) -> Option<&OutputBinding> {
        match self {
            Binding::Output(output_binding) => Some(output_binding),
            _ => None,
        }
    }
    pub fn as_output_binding_mut(&mut self) -> Option<&mut OutputBinding> {
        match self {
            Binding::Output(output_binding) => Some(output_binding),
            _ => None,
        }
    }

    pub fn is_output_binding(&self) -> bool {
        self.as_output_binding().is_some()
    }
    pub fn is_const(&self) -> bool {
        *self == Binding::Const
    }

    pub fn is_some(&self) -> bool {
        match self {
            Binding::None => false,
            Binding::Const | Binding::Output(_) => true
        }
    }
}

impl SubGraph {
    pub fn new() -> SubGraph {
        SubGraph {
            self_id: Uuid::new_v4(),

            name: "".to_string(),
            inputs: vec![],
            outputs: vec![],
        }
    }

    pub fn id(&self) -> Uuid {
        self.self_id
    }
}

impl FunctionBehavior {
    pub fn toggle(&mut self) {
        *self = match *self {
            FunctionBehavior::Active => FunctionBehavior::Passive,
            FunctionBehavior::Passive => FunctionBehavior::Active,
        };
    }
}
