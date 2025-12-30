use hashbrown::HashMap;
use serde::{Deserialize, Serialize};

use crate::data::StaticValue;
use crate::function::{Func, FuncBehavior, FuncId};
use common::{deserialize, serialize, FileFormat, SerdeFormatResult};
use common::{id_type, is_debug};

id_type!(NodeId);

#[derive(Clone, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct OutputBinding {
    pub output_node_id: NodeId,
    pub output_idx: usize,
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

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum NodeBehavior {
    #[default]
    // should execute depending on the function's behavior
    // will be executed always for impure functions
    // for pure functions, only on input change
    AsFunction,
    Once,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub func_id: FuncId,
    pub name: String,

    pub behavior: NodeBehavior,
    pub terminal: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<Event>,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
}

impl Graph {
    pub fn add(&mut self, node: Node) {
        match self.nodes.iter().position(|n| n.id == node.id) {
            Some(index) => self.nodes[index] = node,
            None => self.nodes.push(node),
        }
    }
    pub fn remove_by_id(&mut self, id: NodeId) {
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

    pub fn by_name(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.name == name)
    }
    pub fn by_name_mut(&mut self, name: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|node| node.name == name)
    }

    pub fn by_id(&self, id: NodeId) -> Option<&Node> {
        assert!(!id.is_nil());
        self.nodes.iter().find(|node| node.id == id)
    }
    pub fn by_id_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        assert!(!id.is_nil());

        self.nodes.iter_mut().find(|node| node.id == id)
    }

    pub fn serialize(&self, format: FileFormat) -> String {
        serialize(self, format)
    }
    pub fn from_file(path: &str) -> anyhow::Result<Graph> {
        let format = FileFormat::from_file_name(path)
            .expect("Failed to infer graph file format from file name");
        let contents = std::fs::read_to_string(path)?;
        Ok(Self::deserialize(&contents, format)?)
    }
    pub fn deserialize(serialized: &str, format: FileFormat) -> SerdeFormatResult<Graph> {
        let graph: Graph = deserialize(serialized, format)?;
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
                if let Binding::Output(output_binding) = &input.binding {
                    assert!(self.by_id(output_binding.output_node_id).is_some());
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
            .map(|func_input| Input {
                binding: func_input
                    .default_value
                    .as_ref()
                    .map_or(Binding::None, |_| Binding::Const),
                const_value: func_input.default_value.clone(),
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
    pub fn from_output_binding(output_node_id: NodeId, output_idx: usize) -> Binding {
        Binding::Output(OutputBinding {
            output_node_id,
            output_idx,
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

pub fn test_graph() -> Graph {
    let mut graph = Graph::default();

    let mult_node_id: NodeId = "579ae1d6-10a3-4906-8948-135cb7d7508b".parse().unwrap();
    let mult_func_id: FuncId = "432b9bf1-f478-476c-a9c9-9a6e190124fc".parse().unwrap();

    let get_a_node_id: NodeId = "5f110618-8faa-4629-8f5d-473c236de7d1".parse().unwrap();
    let get_a_func_id: FuncId = "d4d27137-5a14-437a-8bb5-b2f7be0941a2".parse().unwrap();

    let get_b_node_id: NodeId = "6fc6b533-c375-451c-ba3a-a14ea217cb30".parse().unwrap();
    let get_b_func_id: FuncId = "a937baff-822d-48fd-9154-58751539b59b".parse().unwrap();

    let sum_node_id: NodeId = "999c4d37-e0eb-4856-be3f-ad2090c84d8c".parse().unwrap();
    let sum_func_id: FuncId = "2d3b389d-7b58-44d9-b3d1-a595765b21a5".parse().unwrap();

    let print_node_id: NodeId = "b88ab7e2-17b7-46cb-bc8e-b428bb45141e".parse().unwrap();
    let print_func_id: FuncId = "f22cd316-1cdf-4a80-b86c-1277acd1408a".parse().unwrap();

    graph.add(Node {
        id: mult_node_id,
        func_id: mult_func_id,
        name: "mult".to_string(),
        behavior: NodeBehavior::AsFunction,
        terminal: false,
        inputs: vec![
            Input {
                binding: Binding::from_output_binding(sum_node_id, 0),
                const_value: None,
            },
            Input {
                binding: Binding::from_output_binding(get_b_node_id, 0),
                const_value: Some(StaticValue::Int(55)),
            },
        ],
        events: vec![],
    });

    graph.add(Node {
        id: get_a_node_id,
        func_id: get_a_func_id,
        name: "get_a".to_string(),
        behavior: NodeBehavior::AsFunction,
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
                binding: Binding::from_output_binding(get_a_node_id, 0),
                const_value: Some(StaticValue::Int(123)),
            },
            Input {
                binding: Binding::from_output_binding(get_b_node_id, 0),
                const_value: Some(StaticValue::Int(12)),
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
            binding: Binding::from_output_binding(mult_node_id, 0),
            const_value: None,
        }],
        events: vec![],
    });

    graph.validate();

    graph
}

#[cfg(test)]
mod tests {
    use crate::data::StaticValue;
    use crate::graph::{Binding, Graph, Input, Node, OutputBinding};
    use common::FileFormat;
    use std::hint::black_box;

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

        let node_id = graph
            .by_name("sum")
            .unwrap_or_else(|| panic!("Node named \"sum\" not found"))
            .id;
        graph.remove_by_id(node_id);

        assert!(graph.by_name("sum").is_none());
        assert_eq!(graph.nodes.len(), 4);

        for input in graph.nodes.iter().flat_map(|node| node.inputs.iter()) {
            if let Some(binding) = input.binding.as_output_binding() {
                assert_ne!(binding.output_node_id, node_id);
            }
        }

        Ok(())
    }
}
