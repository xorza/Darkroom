use crate::graph::Graph;

#[no_mangle]
pub extern "C" fn graph_new() -> *mut Graph {
    let graph = Graph::default();
    Box::into_raw(Box::new(graph))
}

#[no_mangle]
pub unsafe extern "C" fn graph_free(graph: *mut Graph) {
    assert!(!graph.is_null());

    let graph = Box::from_raw(graph);
    drop(graph);
}

#[test]
fn test_graph_new() {
    let graph = graph_new();
    unsafe {
        graph_free(graph);
    }
}