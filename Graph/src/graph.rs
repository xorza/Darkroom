use serde::{Deserialize, Serialize};
use uuid::Uuid;

use common::id_type;

use crate::data::{DataType, StaticValue};
use crate::function::{Function, FunctionId};
use crate::subgraph::{SubGraph, SubGraphId};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum FunctionBehavior {
    #[default]
    Active,
    Passive,
}

id_type!(NodeId);

#[derive(Clone, Serialize, Deserialize)]
pub struct Node {
    self_id: NodeId,

    pub function_id: FunctionId,

    pub name: String,
    pub behavior: FunctionBehavior,
    pub is_output: bool,
    pub should_cache_outputs: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<Output>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subgraph_id: Option<SubGraphId>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Output {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputBinding {
    pub output_node_id: NodeId,
    pub output_index: u32,
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
    pub const_value: Option<StaticValue>,
}


#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Graph {
    nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    subgraphs: Vec<SubGraph>,
}


impl Graph {
    pub fn nodes(&self) -> &[Node] {
        self.nodes.as_slice()
    }
    pub fn nodes_mut(&mut self) -> &mut [Node] {
        self.nodes.as_mut_slice()
    }

    pub fn add_node(&mut self, node: Node) {
        match self.nodes.iter().position(|n| n.self_id == node.self_id) {
            Some(index) => self.nodes[index] = node,
            None => self.nodes.push(node),
        }
    }
    pub fn remove_node_by_id(&mut self, id: NodeId) {
        assert_ne!(id.0, Uuid::nil());

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

    pub fn node_by_id(&self, id: NodeId) -> Option<&Node> {
        assert!(!id.is_nil());

        self.nodes
            .iter()
            .find(|node| node.self_id == id)
    }
    pub fn node_by_id_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        assert!(!id.is_nil());

        self.nodes
            .iter_mut()
            .find(|node| node.self_id == id)
    }
    pub fn nodes_by_subgraph_id(&self, subgraph_id: SubGraphId) -> Vec<&Node> {
        assert!(!subgraph_id.is_nil());

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
            if node.self_id == NodeId::nil() {
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
                    if node.subgraph_id != Some(subgraph.id()) {
                        return Err(anyhow::Error::msg("Subgraph input connected to an external node"));
                    }

                    // let input = node.inputs.get(connection.subnode_input_index as usize)
                    //     .ok_or(anyhow::Error::msg("Subgraph input connected to a non-existent input"))?;
                    // if !DataType::can_assign(&subinput.data_type, &input.data_type) {
                    //     return Err(anyhow::Error::msg("Subgraph input connected to a node input with an incompatible data type"));
                    // }
                }
            }

            for suboutput in subgraph.outputs.iter() {
                let node = self.node_by_id(suboutput.subnode_id)
                    .ok_or(anyhow::Error::msg("Subgraph output connected to a non-existent node"))?;
                if node.subgraph_id != Some(subgraph.id()) {
                    return Err(anyhow::Error::msg("Subgraph output connected to an external node"));
                }

                // let output = node.outputs.get(suboutput.subnode_output_index as usize)
                //     .ok_or(anyhow::Error::msg("Subgraph output connected to a non-existent output"))?;
                // if !DataType::can_assign(&suboutput.data_type, &output.data_type) {
                //     return Err(anyhow::Error::msg("Subgraph output connected to a node output with an incompatible data type"));
                // }
            }
        }

        Ok(())
    }


    pub(crate) fn subgraphs(&self) -> &Vec<SubGraph> {
        &self.subgraphs
    }
    pub(crate) fn subgraphs_mut(&mut self) -> &mut Vec<SubGraph> {
        &mut self.subgraphs
    }
}

impl Node {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Node {
        Node {
            self_id: NodeId::unique(),
            function_id: FunctionId::nil(),
            name: "".to_string(),
            behavior: FunctionBehavior::Active,
            is_output: false,
            should_cache_outputs: false,
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
                is_required: func_input.is_required,
                binding: func_input.default_value.as_ref().map_or(Binding::None, |_| Binding::Const),
                const_value: func_input.default_value.clone(),
            }
        }).collect();

        let outputs: Vec<Output> = function.outputs.iter().map(|output| {
            Output {
                name: output.name.clone(),
                data_type: output.data_type.clone(),
            }
        }).collect();

        Node {
            self_id: NodeId::unique(),
            function_id: function.self_id,
            name: function.name.clone(),
            behavior: FunctionBehavior::Active,
            should_cache_outputs: false,
            is_output: false,
            inputs,
            outputs,
            subgraph_id: None,
        }
    }

    pub fn id(&self) -> NodeId {
        self.self_id
    }
}

impl Binding {
    pub fn from_output_binding(output_node_id: NodeId, output_index: u32) -> Binding {
        Binding::Output(OutputBinding {
            output_node_id,
            output_index,
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

impl FunctionBehavior {
    pub fn toggle(&mut self) {
        *self = match *self {
            FunctionBehavior::Active => FunctionBehavior::Passive,
            FunctionBehavior::Passive => FunctionBehavior::Active,
        };
    }
}
