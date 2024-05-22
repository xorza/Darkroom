use graph::graph::Graph;

use crate::{FfiBuf, FfiStr, Id};

#[repr(C)]
struct Input {
    a: u8,
}

#[repr(C)]
struct Node {
    id: Id,
    func_id: Id,
    name: FfiStr,
    is_output: bool,
    cache_outputs: bool,
    inputs: FfiBuf,
    events: FfiBuf,
}

impl From<&graph::graph::Node> for Node {
    fn from(node: &graph::graph::Node) -> Self {
        Node {
            id: node.id.as_uuid().into(),
            func_id: node.func_id.as_uuid().into(),
            name: node.name.clone().into(),
            is_output: node.is_output,
            cache_outputs: node.cache_outputs,
            inputs: FfiBuf::default(),
            events: FfiBuf::default(),
        }
    }
}

impl From<uuid::Uuid> for Id {
    fn from(value: uuid::Uuid) -> Self {
        Id(value.to_string().into())
    }
}

#[no_mangle]
extern "C" fn get_nodes() -> FfiBuf {
    let graph = Graph::from_yaml(include_str!("../../test_resources/test_graph.yml")).unwrap();
    graph
        .nodes()
        .iter()
        .map(Node::from)
        .collect::<Vec<Node>>()
        .into()
}

#[no_mangle]
extern "C" fn dummy1(_a: Node, _b: Input) {}
