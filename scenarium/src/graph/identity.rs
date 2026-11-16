//! How an authored graph names things: the uuid identities ([`NodeId`],
//! [`FuncId`], [`PortId`]), the addresses that pair a node with a port's id ([`InputPort`],
//! [`OutputPort`], [`EventPort`]), and the crate-internal [`FuncRef`] that pairs a func's id
//! with the signature of its ports for the compiled program.
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
use crate::graph::ports::Ports;

/// A node's id, unique across a whole document, so a bare one is an unambiguous address.
pub type NodeId = Id<Node>;
/// A declaration's id in the library, not anything in a document.
pub type FuncId = Id<Func>;
/// A port's id, unique among the ports of its side of a declaration. A func numbers its ports,
/// so a reorder and a rename keep it.
pub type PortId = Id<Ports>;

/// The implementation a compiled node runs: its func's id and the signature of its ports, which
/// a node's state and cached values are owned by. Internal to the crate: a node names its func by
/// its [`FuncId`] alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// Address of a producer node's output port, the source side of a `Binding::Wire`, by the id
/// its declaration gives the port.
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct OutputPort {
    pub node_id: NodeId,
    pub port: PortId,
}

impl OutputPort {
    pub const fn new(node_id: NodeId, port: PortId) -> Self {
        Self { node_id, port }
    }
}

/// Address of an emitter node's event, by the id its declaration gives it.
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct EventPort {
    pub node_id: NodeId,
    pub event: PortId,
}

impl EventPort {
    pub const fn new(node_id: NodeId, event: PortId) -> Self {
        Self { node_id, event }
    }
}

/// Address of a consumer node's input port; keys `Graph.bindings`. Distinct from `OutputPort`
/// so source and sink can't be confused.
#[derive(
    Clone, Copy, Default, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct InputPort {
    pub node_id: NodeId,
    pub port: PortId,
}

impl InputPort {
    pub const fn new(node_id: NodeId, port: PortId) -> Self {
        Self { node_id, port }
    }
}
