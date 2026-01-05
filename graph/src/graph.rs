use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use hashbrown::HashSet;
use serde::{Deserialize, Serialize};

use crate::data::StaticValue;
use crate::function::{Func, FuncId, FuncLib};
use common::{deserialize, is_false, serialize, FileFormat, SerdeFormatResult};
use common::{id_type, is_debug};

id_type!(NodeId);

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct PortAddress {
    pub target_id: NodeId,
    pub port_idx: usize,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub enum Binding {
    #[default]
    None,
    Const(StaticValue),
    Bind(PortAddress),
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Input {
    #[serde(default, skip_serializing_if = "Binding::is_none")]
    pub binding: Binding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<StaticValue>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Event {
    pub subscribers: Vec<NodeId>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum NodeBehavior {
    #[default]
    AsFunction,
    Once,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub func_id: FuncId,
    pub name: String,

    #[serde(default, skip_serializing_if = "NodeBehavior::is_default")]
    pub behavior: NodeBehavior,

    #[serde(default, skip_serializing_if = "is_false")]
    pub terminal: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Event>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: KeyIndexVec<NodeId, Node>,
}

impl Graph {
    pub fn add(&mut self, node: Node) {
        match self.nodes.iter().position(|n| n.id == node.id) {
            Some(idx) => self.nodes[idx] = node,
            None => self.nodes.push(node),
        }
    }
    pub fn remove_by_id(&mut self, id: NodeId) {
        assert!(!id.is_nil());

        self.nodes.remove_by_key(&id);

        self.nodes
            .iter_mut()
            .flat_map(|node| node.inputs.iter_mut())
            .filter_map(|input| match &input.binding {
                Binding::Bind(output_binding) if output_binding.target_id == id => Some(input),
                _ => None,
            })
            .for_each(|input| {
                input.binding = Binding::None;
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

    pub fn serialize(&self, format: FileFormat) -> String {
        serialize(self, format)
    }
    pub fn deserialize(serialized: &str, format: FileFormat) -> SerdeFormatResult<Graph> {
        let graph: Self = deserialize(serialized, format)?;
        graph.validate();
        Ok(graph)
    }

    pub fn validate(&self) {
        if is_debug() {
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
            name: "".to_string(),
            behavior: NodeBehavior::AsFunction,
            terminal: false,
            inputs: vec![],
            events: vec![],
        }
    }
}

impl Node {
    pub fn from_function(func: &Func) -> Node {
        let inputs: Vec<Input> = func
            .inputs
            .iter()
            .map(|input| Input {
                binding: Binding::None,
                default_value: input.default_value.clone(),
            })
            .collect();

        let events: Vec<Event> = func.events.iter().map(|_event| Event::default()).collect();

        Node {
            id: NodeId::unique(),
            func_id: func.id,
            name: func.name.clone(),
            behavior: NodeBehavior::AsFunction,
            terminal: func.terminal(),
            inputs,
            events,
        }
    }
}

impl Binding {
    pub fn unwrap_bind(&self) -> &PortAddress {
        match self {
            Binding::Bind(port_address) => port_address,
            Binding::None => {
                panic!("expected Binding::Bind, got None")
            }
            Binding::Const(_) => {
                panic!("expected Binding::Bind, got Const")
            }
        }
    }
    pub fn unwrap_const(&self) -> &StaticValue {
        match self {
            Binding::Const(const_value) => const_value,
            Binding::None => {
                panic!("expected Binding::Const, got None")
            }
            Binding::Bind(_) => {
                panic!("expected Binding::Const, got Bind")
            }
        }
    }
    pub fn as_output_binding(&self) -> Option<&PortAddress> {
        match self {
            Binding::Bind(output_binding) => Some(output_binding),
            _ => None,
        }
    }
    pub fn as_output_binding_mut(&mut self) -> Option<&mut PortAddress> {
        match self {
            Binding::Bind(output_binding) => Some(output_binding),
            _ => None,
        }
    }
    pub fn as_const(&self) -> Option<&StaticValue> {
        match self {
            Binding::Const(static_value) => Some(static_value),
            _ => None,
        }
    }
    pub fn as_const_mut(&mut self) -> Option<&mut StaticValue> {
        match self {
            Binding::Const(static_value) => Some(static_value),
            _ => None,
        }
    }

    pub fn is_output_binding(&self) -> bool {
        matches!(self, Binding::Bind(_))
    }
    pub fn is_const(&self) -> bool {
        matches!(self, Binding::Const(_))
    }
    pub fn is_some(&self) -> bool {
        !self.is_none()
    }
    pub fn is_none(&self) -> bool {
        matches!(self, Binding::None)
    }
}

impl NodeBehavior {
    pub fn is_default(value: &Self) -> bool {
        matches!(value, NodeBehavior::AsFunction)
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
impl From<StaticValue> for Binding {
    fn from(value: StaticValue) -> Self {
        Binding::Const(value)
    }
}

pub fn test_graph() -> Graph {
    let mut graph = Graph::default();

    let mult_node_id: NodeId = "579ae1d6-10a3-4906-8948-135cb7d7508b".into();
    let mult_func_id: FuncId = "432b9bf1-f478-476c-a9c9-9a6e190124fc".into();

    let get_a_node_id: NodeId = "5f110618-8faa-4629-8f5d-473c236de7d1".into();
    let get_a_func_id: FuncId = "d4d27137-5a14-437a-8bb5-b2f7be0941a2".into();

    let get_b_node_id: NodeId = "6fc6b533-c375-451c-ba3a-a14ea217cb30".into();
    let get_b_func_id: FuncId = "a937baff-822d-48fd-9154-58751539b59b".into();

    let sum_node_id: NodeId = "999c4d37-e0eb-4856-be3f-ad2090c84d8c".into();
    let sum_func_id: FuncId = "2d3b389d-7b58-44d9-b3d1-a595765b21a5".into();

    let print_node_id: NodeId = "b88ab7e2-17b7-46cb-bc8e-b428bb45141e".into();
    let print_func_id: FuncId = "f22cd316-1cdf-4a80-b86c-1277acd1408a".into();

    graph.add(Node {
        id: mult_node_id,
        func_id: mult_func_id,
        name: "mult".to_string(),
        behavior: NodeBehavior::AsFunction,
        terminal: false,
        inputs: vec![
            Input {
                binding: (sum_node_id, 0).into(),
                default_value: None,
            },
            Input {
                binding: (get_b_node_id, 0).into(),
                default_value: None,
            },
        ],
        events: vec![],
    });

    graph.add(Node {
        id: get_a_node_id,
        func_id: get_a_func_id,
        name: "get_a".to_string(),
        behavior: NodeBehavior::Once,
        terminal: false,
        inputs: vec![],
        events: vec![],
    });

    graph.add(Node {
        id: get_b_node_id,
        func_id: get_b_func_id,
        name: "get_b".to_string(),
        behavior: NodeBehavior::Once,
        terminal: false,
        inputs: vec![],
        events: vec![],
    });

    graph.add(Node {
        id: sum_node_id,
        func_id: sum_func_id,
        name: "sum".to_string(),
        behavior: NodeBehavior::AsFunction,
        terminal: false,
        inputs: vec![
            Input {
                binding: (get_a_node_id, 0).into(),
                default_value: None,
            },
            Input {
                binding: (get_b_node_id, 0).into(),
                default_value: None,
            },
        ],
        events: vec![],
    });

    graph.add(Node {
        id: print_node_id,
        func_id: print_func_id,
        name: "print".to_string(),
        behavior: NodeBehavior::AsFunction,
        terminal: true,
        inputs: vec![Input {
            binding: (mult_node_id, 0).into(),
            default_value: None,
        }],
        events: vec![],
    });

    graph.validate();

    graph
}

#[cfg(test)]
mod tests {
    use crate::graph::Graph;
    use common::FileFormat;

    #[test]
    fn roundtrip_serialization() -> anyhow::Result<()> {
        let graph = super::test_graph();

        for format in [FileFormat::Yaml, FileFormat::Json, FileFormat::Lua] {
            let serialized = graph.serialize(format);
            let deserialized = Graph::deserialize(serialized.as_str(), format)?;
            let serialized_again = deserialized.serialize(format);
            assert_eq!(serialized, serialized_again);
        }

        Ok(())
    }

    #[test]
    fn node_remove_test() -> anyhow::Result<()> {
        let mut graph = super::test_graph();

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
        let graph = super::test_graph();

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

        assert_eq!(graph.dependent_nodes(&get_a), vec![mult, sum, print]);
        assert_eq!(graph.dependent_nodes(&get_b), vec![mult, sum, print]);
        assert_eq!(graph.dependent_nodes(&sum), vec![mult, print]);
        assert_eq!(graph.dependent_nodes(&mult), vec![print]);
        assert!(graph.dependent_nodes(&print).is_empty());
    }
}
