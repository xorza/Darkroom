//! How an authored graph names things: the uuid identities ([`NodeId`],
//! [`FuncId`]) and the addresses that pair one with a port index
//! ([`InputPort`], [`OutputPort`], [`EventPort`]).
//!
//! Gathered here rather than each sitting with the type it names, so "what
//! can this model address?" is one file. They carry no authoring state, so
//! the execution side can speak about nodes and ports without reaching into
//! the model behind them — [`error`](crate::graph::error) reports the ports a
//! fault touched, and a compiled program names its nodes and ports with these
//! same types: a graph is flat, so there is no second identity space to
//! derive.

use ::common::id_type;
use ::serde::{Deserialize, Serialize};

// A `NodeId` is unique across a whole document, so a bare one is an
// unambiguous address. `FuncId` names a declaration in the library rather than
// anything in a document.
id_type!(NodeId);
id_type!(FuncId);

/// Address of a producer node's output port — the source side of a data
/// binding (`Binding::Bind`).
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct OutputPort {
    pub node_id: NodeId,
    pub port_idx: usize,
}

impl OutputPort {
    pub fn new(node_id: NodeId, port_idx: usize) -> Self {
        Self { node_id, port_idx }
    }
}

/// Address of an emitter node's event port. The event-side sibling of
/// [`OutputPort`]: a node plus a declaration-order index, naming where an
/// event fires from rather than where a value comes out.
///
/// Lives here rather than in the execution internals because it names the
/// same thing on both sides — a graph is flat, so a node in a compiled
/// program is the authored node, and its event ports are the func's.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct EventPort {
    pub node_id: NodeId,
    pub event_idx: usize,
}

impl EventPort {
    pub fn new(node_id: NodeId, event_idx: usize) -> Self {
        Self { node_id, event_idx }
    }
}

/// Address of a consumer node's input port. Keys a node's data binding in
/// `Graph.bindings`, and reports unsatisfied inputs
/// (the execution outcome's missing-input list) / edges the editor's breaker severs.
/// Distinct from `OutputPort` so source/sink intent can't be confused.
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct InputPort {
    pub node_id: NodeId,
    pub port_idx: usize,
}

impl InputPort {
    pub fn new(node_id: NodeId, port_idx: usize) -> Self {
        Self { node_id, port_idx }
    }
}
