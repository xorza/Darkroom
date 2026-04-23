use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use hashbrown::HashSet;
use serde::{Deserialize, Serialize};

use crate::data::{DataType, StaticValue};
use crate::function::{Func, FuncId, FuncLib};
use common::{Result, SerdeFormat, deserialize, serialize};
use common::{id_type, is_debug};

id_type!(NodeId);

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortAddress {
    pub target_id: NodeId,
    pub port_idx: usize,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Binding {
    #[default]
    None,
    Const(StaticValue),
    Bind(PortAddress),
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Input {
    pub name: String,

    #[serde(default)]
    pub binding: Binding,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub name: String,
    pub subscribers: Vec<NodeId>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum NodeBehavior {
    #[default]
    AsFunction,
    Once,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub id: NodeId,
    pub func_id: FuncId,
    pub name: String,

    #[serde(default)]
    pub behavior: NodeBehavior,

    #[serde(default)]
    pub inputs: Vec<Input>,
    #[serde(default)]
    pub events: Vec<Event>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Graph {
    pub nodes: KeyIndexVec<NodeId, Node>,
}

impl Graph {
    pub fn add(&mut self, node: Node) {
        self.nodes.add(node);
    }
    pub fn remove_by_id(&mut self, id: NodeId) {
        assert!(!id.is_nil());

        self.nodes.remove_by_key(&id);

        self.nodes
            .iter_mut()
            .flat_map(|node| node.inputs.iter_mut())
            .for_each(|input| match &input.binding {
                Binding::Bind(output_binding) if output_binding.target_id == id => {
                    input.binding = Binding::None;
                }
                _ => {}
            });

        self.nodes
            .iter_mut()
            .flat_map(|node| node.events.iter_mut())
            .for_each(|event| {
                event.subscribers.retain(|sub| *sub != id);
            });
    }

    pub fn by_name(&self, name: &str) -> Option<&Node> {
        assert!(!name.is_empty());
        self.nodes.iter().find(|node| node.name == name)
    }
    pub fn by_name_mut(&mut self, name: &str) -> Option<&mut Node> {
        assert!(!name.is_empty());
        self.nodes.iter_mut().find(|node| node.name == name)
    }

    pub fn by_id(&self, id: &NodeId) -> Option<&Node> {
        assert!(!id.is_nil());
        self.nodes.by_key(id)
    }
    pub fn by_id_mut(&mut self, id: &NodeId) -> Option<&mut Node> {
        assert!(!id.is_nil());
        self.nodes.by_key_mut(id)
    }

    pub fn dependent_nodes(&self, node_id: &NodeId) -> Vec<NodeId> {
        assert!(!node_id.is_nil());
        assert!(
            self.by_id(node_id).is_some(),
            "node must exist to find dependents"
        );

        let mut seen = HashSet::new();
        let mut stack = vec![*node_id];

        while let Some(current) = stack.pop() {
            for node in self.nodes.iter() {
                let depends = node.inputs.iter().any(|input| {
                    matches!(
                        &input.binding,
                        Binding::Bind(binding) if binding.target_id == current
                    )
                });
                if depends && seen.insert(node.id) {
                    stack.push(node.id);
                }
            }
        }

        let mut ordered = Vec::with_capacity(seen.len());
        for node in self.nodes.iter() {
            if seen.contains(&node.id) {
                ordered.push(node.id);
            }
        }
        ordered
    }

    pub fn serialize(&self, format: SerdeFormat) -> Vec<u8> {
        serialize(self, format)
    }
    pub fn deserialize(serialized: &[u8], format: SerdeFormat) -> Result<Graph> {
        let graph: Self = deserialize(serialized, format)?;
        graph.validate();
        Ok(graph)
    }

    pub fn validate(&self) {
        if !is_debug() {
            return;
        }

        for node in self.nodes.iter() {
            assert_ne!(node.id, NodeId::nil());
            assert_ne!(node.func_id, FuncId::nil());

            for input in node.inputs.iter() {
                if let Binding::Bind(output_binding) = &input.binding {
                    assert!(self.by_id(&output_binding.target_id).is_some());
                }
            }
        }
    }

    pub fn validate_with(&self, func_lib: &FuncLib) {
        if !is_debug() {
            return;
        }

        self.validate();

        for node in self.nodes.iter() {
            let func = func_lib.by_id(&node.func_id).unwrap();
            assert_eq!(node.inputs.len(), func.inputs.len());
            assert_eq!(node.events.len(), func.events.len());

            for input in node.inputs.iter() {
                if let Binding::Bind(port_address) = &input.binding {
                    let output_node = self.by_id(&port_address.target_id).unwrap();
                    let output_func = func_lib.by_id(&output_node.func_id).unwrap();
                    assert!(port_address.port_idx < output_func.outputs.len());
                }
            }
        }
    }
}

impl Default for Node {
    fn default() -> Self {
        Node {
            id: NodeId::unique(),
            func_id: FuncId::nil(),
            name: String::new(),
            behavior: NodeBehavior::AsFunction,
            inputs: vec![],
            events: vec![],
        }
    }
}

impl From<&Func> for Node {
    fn from(func: &Func) -> Self {
        let inputs: Vec<Input> = func
            .inputs
            .iter()
            .map(|func_input| Input {
                binding: match &func_input.default_value {
                    Some(default) => Binding::Const(default.clone()),
                    None => Binding::None,
                },
                name: func_input.name.clone(),
            })
            .collect();

        let events: Vec<Event> = func
            .events
            .iter()
            .map(|func_event| Event {
                name: func_event.name.clone(),
                subscribers: Vec::default(),
            })
            .collect();

        Node {
            id: NodeId::unique(),
            func_id: func.id,
            name: func.name.clone(),
            behavior: func.node_default_behavior,
            inputs,
            events,
        }
    }
}

impl Binding {
    pub fn as_output_binding(&self) -> Option<&PortAddress> {
        match self {
            Binding::Bind(output_binding) => Some(output_binding),
            _ => None,
        }
    }

    pub fn is_some(&self) -> bool {
        !self.is_none()
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Binding::None)
    }
}

impl KeyIndexKey<NodeId> for Node {
    fn key(&self) -> &NodeId {
        &self.id
    }
}

impl From<PortAddress> for Binding {
    fn from(value: PortAddress) -> Self {
        Binding::Bind(value)
    }
}

impl From<(NodeId, usize)> for Binding {
    fn from((output_node_id, output_idx): (NodeId, usize)) -> Self {
        Binding::Bind(PortAddress {
            target_id: output_node_id,
            port_idx: output_idx,
        })
    }
}

impl From<&DataType> for Binding {
    fn from(value: &DataType) -> Self {
        Binding::Const(value.into())
    }
}

impl From<StaticValue> for Binding {
    fn from(value: StaticValue) -> Self {
        Binding::Const(value)
    }
}

impl From<i64> for Binding {
    fn from(value: i64) -> Self {
        Binding::Const(value.into())
    }
}

impl NodeBehavior {
    pub fn toggle(&mut self) {
        match self {
            NodeBehavior::AsFunction => *self = NodeBehavior::Once,
            NodeBehavior::Once => *self = NodeBehavior::AsFunction,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::graph::Graph;
    use crate::testing::test_graph;
    use common::SerdeFormat;

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let graph = test_graph();

        for format in SerdeFormat::all_formats_for_testing() {
            let serialized = graph.serialize(format);
            let deserialized = Graph::deserialize(&serialized, format)?;
            let serialized_again = deserialized.serialize(format);
            assert_eq!(serialized, serialized_again);
        }

        let bin = graph.serialize(SerdeFormat::Bitcode);
        let deserialized = Graph::deserialize(&bin, SerdeFormat::Bitcode)?;
        assert_eq!(graph, deserialized);

        Ok(())
    }

    #[test]
    fn node_remove_test() -> anyhow::Result<()> {
        let mut graph = test_graph();

        let node_id = graph.by_name("sum").unwrap().id;
        graph.remove_by_id(node_id);

        assert!(graph.by_name("sum").is_none());
        assert_eq!(graph.nodes.len(), 4);

        for input in graph.nodes.iter().flat_map(|node| node.inputs.iter()) {
            if let Some(binding) = input.binding.as_output_binding() {
                assert_ne!(binding.target_id, node_id);
            }
        }

        Ok(())
    }

    #[test]
    fn dependent_nodes() {
        let graph = test_graph();

        let get_id = |name: &str| {
            graph
                .by_name(name)
                .unwrap_or_else(|| panic!("Node named \"{name}\" not found"))
                .id
        };

        let get_a = get_id("get_a");
        let get_b = get_id("get_b");
        let sum = get_id("sum");
        let mult = get_id("mult");
        let print = get_id("print");

        assert_eq!(graph.dependent_nodes(&get_a), vec![sum, mult, print]);
        assert_eq!(graph.dependent_nodes(&get_b), vec![sum, mult, print]);
        assert_eq!(graph.dependent_nodes(&sum), vec![mult, print]);
        assert_eq!(graph.dependent_nodes(&mult), vec![print]);
        assert!(graph.dependent_nodes(&print).is_empty());
    }
}
