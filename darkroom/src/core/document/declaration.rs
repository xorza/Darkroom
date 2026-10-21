//! What a node's func declares about its ports and events. Whether an index
//! names one is the rule the edit gate and a document load both hold wiring
//! to; a const-only input is the gate's alone, since a load that kept such a
//! wire fails every compile with a message rather than quietly.
//!
//! A node whose func the library does not hold has no declaration to check
//! against, and passes: it loads as a missing stub with the wiring it was saved
//! with, and nothing can author new wiring onto it, since it shows no ports.

use scenarium::{Library, Node};

pub(crate) fn admits_input(node: &Node, library: &Library, port_idx: usize) -> bool {
    node.func(library)
        .is_none_or(|func| port_idx < func.inputs.len())
}

pub(crate) fn admits_output(node: &Node, library: &Library, port_idx: usize) -> bool {
    node.func(library)
        .is_none_or(|func| port_idx < func.outputs.len())
}

pub(crate) fn admits_event(node: &Node, library: &Library, event_idx: usize) -> bool {
    node.func(library)
        .is_none_or(|func| event_idx < func.events.len())
}

/// Whether input `port_idx` is declared const-only, so no wire may land on it.
pub(crate) fn const_only(node: &Node, library: &Library, port_idx: usize) -> bool {
    node.func(library)
        .and_then(|func| func.inputs.get(port_idx))
        .is_some_and(|input| input.const_only)
}
