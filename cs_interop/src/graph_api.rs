use std::ffi::c_void;

use graph::function::FuncId;
use graph::graph::NodeId;

use crate::{get_context, FfiBuf};
use crate::utils::FfiUuid;


#[no_mangle]
extern "C" fn get_graph(ctx: *mut c_void) -> FfiBuf {
    let graph = &get_context(ctx).graph;

    serde_json::to_string(graph).unwrap().into()
}

#[no_mangle]
unsafe extern "C" fn add_node(ctx: *mut c_void, func_id: FfiUuid) {
    let context = get_context(ctx);

    let id: FuncId = uuid::Uuid::from(func_id).into();
    let func = context.func_lib.func_by_id(id).unwrap();
    let node = graph::graph::Node::from_function(func);
    context.graph.add_node(node);
}

#[no_mangle]
unsafe extern "C" fn remove_node(ctx: *mut c_void, node_id: FfiUuid) {
    let context = get_context(ctx);

    let node_id: NodeId = uuid::Uuid::from(node_id).into();
    context.graph.remove_node_by_id(node_id);
}