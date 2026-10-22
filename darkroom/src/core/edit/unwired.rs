//! What an input holds once the wire into it goes.

use scenarium::{Binding, Graph, InputPort, Library};

/// What `port` holds once the wire into it goes: its declared default, so a
/// preset input shows its pick again and a knob its value; else nothing.
pub(crate) fn binding(graph: &Graph, library: &Library, port: InputPort) -> Option<Binding> {
    let node = graph
        .find(port.node_id)
        .expect("an input that loses its wire is on a graph node");
    node.func(library).inputs()[port.port_idx]
        .default_value
        .clone()
        .map(Binding::Const)
}
