use std::ffi::c_void;
use crate::{get_context, FfiBuf};


#[no_mangle]
extern "C" fn get_graph(ctx: *mut c_void) -> FfiBuf {
    let graph = &get_context(ctx).graph;

    graph.to_yaml()
        .unwrap()
        .into()
}




