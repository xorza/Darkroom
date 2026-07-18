//! Identity and interface types for graph instances.

use common::id_type;
use serde::{Deserialize, Serialize};

use crate::graph::NodeId;

id_type!(GraphId);

/// One outgoing event re-exported from an interior emitter.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphEvent {
    pub name: String,
    pub emitter: NodeId,
    pub emitter_event_idx: usize,
}

/// Registry selected by a graph-instance node.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum GraphLink {
    Shared(GraphId),
    Local(GraphId),
}

impl GraphLink {
    pub fn id(&self) -> GraphId {
        match self {
            GraphLink::Shared(id) | GraphLink::Local(id) => *id,
        }
    }
}

#[cfg(test)]
mod tests;
