//! How an authored graph names things: the uuid identities ([`NodeId`],
//! [`FuncId`]), the addresses that pair one with a port index ([`InputPort`],
//! [`OutputPort`], [`EventPort`]), and the crate-internal [`FuncRef`] that pairs
//! a func's id with the signature of its ports.
//!
//! Gathered here so "what can this model address?" is one file. They carry no authoring state,
//! so the execution side, and [`error`](crate::graph::error)'s reports, name nodes and ports
//! without reaching into the model behind them.
//!
//! The three port addresses are the same shape on purpose and are written out rather than
//! generated, so `pub struct InputPort` is greppable. They carry **one derive list, in one
//! order**, so none is missing a bound its siblings have.

use ::serde::{Deserialize, Serialize};
use common::Id;

use crate::graph::func::Func;
use crate::graph::func::signature::FuncSignature;
use crate::graph::node::Node;

/// A node's id, unique across a whole document, so a bare one is an unambiguous address.
pub type NodeId = Id<Node>;
/// A declaration's id in the library, not anything in a document.
pub type FuncId = Id<Func>;

/// The func a node is authored against: the declaration's id, and the
/// signature of the ports the node's wiring is indexed by.
///
/// A func answers to a node only when both match, so a func whose ports moved
/// under a saved node reads as absent rather than as the one it was wired to.
/// Internal to the crate: hosts name a func by its [`FuncId`] alone.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub(crate) struct FuncRef {
    id: FuncId,
    signature: FuncSignature,
}

impl FuncRef {
    pub(crate) const fn new(id: FuncId, signature: FuncSignature) -> Self {
        Self { id, signature }
    }

    pub(crate) const fn id(self) -> FuncId {
        self.id
    }

    pub(crate) const fn signature(self) -> FuncSignature {
        self.signature
    }
}

/// Address of a producer node's output port, the source side of a `Binding::Wire`.
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct OutputPort {
    pub node_id: NodeId,
    pub port_idx: usize,
}

impl OutputPort {
    pub const fn new(node_id: NodeId, port_idx: usize) -> Self {
        Self { node_id, port_idx }
    }
}

/// Address of an emitter node's event port: where an event fires from, by declaration-order
/// index.
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct EventPort {
    pub node_id: NodeId,
    pub event_idx: usize,
}

impl EventPort {
    pub const fn new(node_id: NodeId, event_idx: usize) -> Self {
        Self { node_id, event_idx }
    }
}

/// Address of a consumer node's input port; keys `Graph.bindings`. Distinct from `OutputPort`
/// so source and sink can't be confused.
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct InputPort {
    pub node_id: NodeId,
    pub port_idx: usize,
}

impl InputPort {
    pub const fn new(node_id: NodeId, port_idx: usize) -> Self {
        Self { node_id, port_idx }
    }
}
