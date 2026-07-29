//! How an authored graph names its nodes and ports.
//!
//! A dependency-free leaf: these types address the authoring model without
//! knowing it, so the execution side can speak about nodes and ports —
//! [`identity`](crate::execution::identity) derives execution ids from
//! [`NodeId`], [`error`](crate::graph::error) reports the ports a fault touched —
//! without reaching into [`graph`](crate::graph) itself.

use ::serde::{Deserialize, Serialize};
use common::id_type;

id_type!(NodeId);

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

/// One event-subscription edge: `subscriber` fires when `emitter`'s event
/// `event_idx` triggers. Ordered (emitter, event_idx, subscriber) so a
/// `BTreeSet` ranges over one emitter-event's subscribers contiguously.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subscription {
    pub emitter: NodeId,
    pub event_idx: usize,
    pub subscriber: NodeId,
}
