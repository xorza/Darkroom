use std::ffi::c_void;

use graph::function::FuncId;
use graph::graph::NodeId;

use crate::{get_context, FfiBuf};
use crate::utils::FfiUuid;

#[repr(C)]
struct FfiInput {
    a: u8,
}

#[repr(C)]
struct FfiNode {
    id: FfiBuf,             // string
    func_id: FfiBuf,        // string
    name: FfiBuf,           // string
    is_output: bool,
    cache_outputs: bool,
    inputs: FfiBuf,         // vector of
    events: FfiBuf,         // vector of ids of subscriber nodes
}

impl From<&graph::graph::Node> for FfiNode {
    fn from(node: &graph::graph::Node) -> Self {
        FfiNode {
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

#[no_mangle]
extern "C" fn get_nodes(ctx: *mut c_void) -> FfiBuf {
    // let graph = Graph::from_yaml(include_str!("../../test_resources/test_graph.yml")).unwrap();

    get_context(ctx)
        .graph
        .nodes()
        .iter()
        .map(FfiNode::from)
        .collect::<Vec<FfiNode>>()
        .into()
}

#[no_mangle]
unsafe extern "C" fn add_node(ctx: *mut c_void, func_id: FfiUuid) -> FfiNode {
    let context = get_context(ctx);

    let id: FuncId = uuid::Uuid::from(func_id).into();
    let func = context.func_lib.func_by_id(id).unwrap();
    let node = graph::graph::Node::from_function(func);
    context.graph.add_node(node);

    context.graph.nodes().last().unwrap().into()
}

#[no_mangle]
unsafe extern "C" fn remove_node(ctx: *mut c_void, node_id: FfiUuid) {
    let context = get_context(ctx);

    let node_id: NodeId = uuid::Uuid::from(node_id).into();
    context.graph.remove_node_by_id(node_id);
}