//! What an input holds once the wire into it goes.

use scenarium::{Binding, Graph, InputPort, Scope};

/// What `port`, an input of `graph` in `scope`, holds once the wire into it goes: its declared
/// default, else nothing.
pub(crate) fn binding(graph: &Graph, scope: Scope<'_>, port: InputPort) -> Option<Binding> {
    let node = graph
        .find(port.node_id)
        .expect("an input that loses its wire is on a graph node");
    node.decl(scope)
        .ports()
        .input(port.port)
        .expect("an input that loses its wire is declared")
        .default_value
        .clone()
        .map(Binding::Const)
}
