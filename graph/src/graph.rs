use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::data::StaticValue;
use crate::function::{Func, FuncId};
use common::id_type;
use common::normalize_string::NormalizeString;

id_type!(NodeId);

#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct OutputBinding {
    pub output_node_id: NodeId,
    pub output_index: u32,
}

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub enum Binding {
    #[default]
    None,
    Const,
    Output(OutputBinding),
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Input {
    pub binding: Binding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub const_value: Option<StaticValue>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Event {
    pub subscribers: Vec<NodeId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub func_id: FuncId,

    pub name: String,
    pub is_output: bool,
    pub cache_outputs: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Event>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub view_pos: glam::Vec2,
    pub view_scale: f32,
}

impl Graph {
    pub fn nodes(&self) -> &[Node] {
        self.nodes.as_slice()
    }
    pub fn nodes_mut(&mut self) -> &mut [Node] {
        self.nodes.as_mut_slice()
    }

    pub fn add_node(&mut self, node: Node) {
        match self.nodes.iter().position(|n| n.id == node.id) {
            Some(index) => self.nodes[index] = node,
            None => self.nodes.push(node),
        }
    }
    pub fn remove_node_by_id(&mut self, id: NodeId) {
        assert!(!id.is_nil());

        self.nodes.retain(|node| node.id != id);

        self.nodes
            .iter_mut()
            .flat_map(|node| node.inputs.iter_mut())
            .filter_map(|input| match &input.binding {
                Binding::Output(output_binding) if output_binding.output_node_id == id => {
                    Some(input)
                }
                _ => None,
            })
            .for_each(|input| {
                input.binding = input
                    .const_value
                    .as_ref()
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
        self.nodes.iter().find(|node| node.id == id)
    }
    pub fn node_by_id_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        assert!(!id.is_nil());

        self.nodes.iter_mut().find(|node| node.id == id)
    }

    pub fn node_index_by_id(&self) -> HashMap<NodeId, usize> {
        let mut map = HashMap::with_capacity(self.nodes.len());
        for (index, node) in self.nodes.iter().enumerate() {
            let prev = map.insert(node.id, index);
            assert!(prev.is_none(), "Duplicate node id detected: {:?}", node.id);
        }
        map
    }

    pub fn to_yaml(&self) -> String {
        serde_yml::to_string(&self)
            .expect("Failed to serialize graph to YAML")
            .normalize()
    }
    pub fn from_yaml_file(path: &str) -> anyhow::Result<Graph> {
        let yaml = std::fs::read_to_string(path)?;
        let graph: Graph = serde_yml::from_str(&yaml)?;

        graph.validate()?;

        Ok(graph)
    }
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Graph> {
        let graph: Graph = serde_yml::from_str(yaml)?;

        graph.validate()?;

        Ok(graph)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        for node in self.nodes.iter() {
            if node.id == NodeId::nil() {
                return Err(anyhow::Error::msg("Node has invalid id"));
            }
            if !node.inputs.is_empty() && node.func_id == FuncId::nil() {
                return Err(anyhow::Error::msg("Node has inputs but no function id"));
            }

            // validate node has valid bindings
            for input in node.inputs.iter() {
                if let Binding::Output(output_binding) = &input.binding {
                    if self.node_by_id(output_binding.output_node_id).is_none() {
                        return Err(anyhow::Error::msg(
                            "Node input connected to a non-existent node",
                        ));
                    }
                }
                if matches!(input.binding, Binding::Const) && input.const_value.is_none() {
                    return Err(anyhow::Error::msg("Const binding missing const_value"));
                }
            }
        }

        Ok(())
    }
}

impl Default for Node {
    fn default() -> Self {
        Node {
            id: NodeId::unique(),
            func_id: FuncId::nil(),
            name: "".to_string(),
            is_output: false,
            cache_outputs: false,
            inputs: vec![],
            events: vec![],
        }
    }
}

impl Node {
    pub fn from_function(function: &Func) -> Node {
        let inputs: Vec<Input> = function
            .inputs
            .iter()
            .map(|func_input| Input {
                binding: func_input
                    .default_value
                    .as_ref()
                    .map_or(Binding::None, |_| Binding::Const),
                const_value: func_input.default_value.clone(),
            })
            .collect();

        let events: Vec<Event> = function
            .events
            .iter()
            .map(|_event| Event::default())
            .collect();

        Node {
            id: NodeId::unique(),
            func_id: function.id,
            name: function.name.clone(),
            is_output: function.is_output,
            cache_outputs: false,
            inputs,
            events,
        }
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
            Binding::Const | Binding::Output(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::StaticValue;
    use crate::graph::{Binding, Graph, Input, Node, OutputBinding};
    use common::yaml_format::reformat_yaml;
    use std::hint::black_box;

    #[test]
    fn graph_to_yaml() -> anyhow::Result<()> {
        let mut graph = Graph::default();
        let mut node1 = Node::default();

        node1.inputs.push(Input {
            binding: Binding::Const,
            const_value: Some(StaticValue::Int(55)),
        });
        let mut node2 = Node::default();
        node2.inputs.push(Input {
            binding: Binding::Output(OutputBinding {
                output_node_id: node1.id,
                output_index: 0,
            }),
            const_value: None,
        });

        graph.add_node(node1);
        graph.add_node(node2);

        let _yaml: String = graph.to_yaml();

        Ok(())
    }

    #[test]
    fn graph_from_yaml() -> anyhow::Result<()> {
        let file_yaml: String = {
            // This trick is used to make yaml formatting consistent
            let str = std::fs::read_to_string("../test_resources/test_graph.yml")?;
            reformat_yaml(str.as_str())?
        };

        let graph = Graph::from_yaml(file_yaml.as_str())?;
        let serialized_yaml: String = graph.to_yaml();
        // std::fs::write("../test_resources/test_graph.yml", &serialized_yaml)?;

        assert_eq!(serialized_yaml, file_yaml);

        let graph = Graph::from_yaml(&serialized_yaml)?;

        black_box(graph);

        Ok(())
    }

    #[test]
    fn node_remove_test() -> anyhow::Result<()> {
        let mut graph = Graph::from_yaml_file("../test_resources/test_graph.yml")?;

        let node_id = graph
            .node_by_name("sum")
            .unwrap_or_else(|| panic!("Node named \"sum\" not found"))
            .id;
        graph.remove_node_by_id(node_id);

        assert!(graph.node_by_name("sum").is_none());
        assert_eq!(graph.nodes().len(), 4);

        for input in graph.nodes().iter().flat_map(|node| node.inputs.iter()) {
            if let Some(binding) = input.binding.as_output_binding() {
                assert_ne!(binding.output_node_id, node_id);
            }
        }

        Ok(())
    }
}
