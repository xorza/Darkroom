use std::ffi::c_void;

use graph::function::FuncId;
use graph::graph::NodeId;

use crate::{get_context, FfiBuf};
use crate::utils::FfiUuid;


#[no_mangle]
extern "C" fn get_nodes(_ctx: *mut c_void) -> FfiBuf {
    // let graph = Graph::from_yaml(include_str!("../../test_resources/test_graph.yml")).unwrap();

    // get_context(ctx)
    //     .graph
    //     .nodes()
    //     .iter()
    //     .map(FfiNode::from)
    //     .collect::<Vec<FfiNode>>()
    //     .into()

    FfiBuf::default()
}

#[no_mangle]
unsafe extern "C" fn add_node(ctx: *mut c_void, func_id: FfiUuid) -> FfiBuf {
    let context = get_context(ctx);

    let id: FuncId = uuid::Uuid::from(func_id).into();
    let func = context.func_lib.func_by_id(id).unwrap();
    let node = graph::graph::Node::from_function(func);
    context.graph.add_node(node);

    // context.graph.nodes().last().unwrap().into()
    FfiBuf::default()
}

#[no_mangle]
unsafe extern "C" fn remove_node(ctx: *mut c_void, node_id: FfiUuid) {
    let context = get_context(ctx);

    let node_id: NodeId = uuid::Uuid::from(node_id).into();
    context.graph.remove_node_by_id(node_id);
}