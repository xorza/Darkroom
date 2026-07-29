//! How an authored graph names things: the uuid identities ([`NodeId`],
//! [`GraphId`], [`FuncId`]) and the addresses that pair one with a port index
//! ([`InputPort`], [`OutputPort`]).
//!
//! Gathered here rather than each sitting with the type it names, so "what
//! can this model address?" is one file. They carry no authoring state, so
//! the execution side can speak about nodes and ports without reaching into
//! the model behind them — [`error`](crate::graph::error) reports the ports a
//! fault touched, and [`identity`](crate::execution::identity) derives the
//! *execution* identity space from these.

use ::serde::{Deserialize, Serialize};
use common::id_type;

// `NodeId` and `GraphId` are unique across a whole document — nested graphs
// included — so a bare one is an unambiguous address. `FuncId` names a
// declaration in the library rather than anything in a document.
id_type!(NodeId);
id_type!(GraphId);
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
