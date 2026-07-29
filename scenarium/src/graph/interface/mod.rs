//! Identity and interface types for graph instances: what a definition
//! *exposes*, as opposed to the body implementing it.

use common::id_type;
use serde::{Deserialize, Serialize};

use crate::graph::address::NodeId;
use crate::node::definition::{FuncInput, FuncOutput};

id_type!(GraphId);

/// What a [`GraphDef`](crate::graph::GraphDef) exposes: its identity in a
/// palette (name, category, library lineage) and the ports an instance node
/// declares.
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphInterface {
    pub name: String,
    pub category: String,

    /// Interface in port order. `inputs[i]` corresponds to `GraphInput`
    /// output port `i`; `outputs[j]` corresponds to `GraphOutput` input port
    /// `j`.
    #[serde(default)]
    pub inputs: Vec<FuncInput>,
    #[serde(default)]
    pub outputs: Vec<FuncOutput>,

    /// Exposed outgoing events re-exported from interior emitters.
    #[serde(default)]
    pub events: Vec<GraphEvent>,

    /// Shared-library graph this value was copied from, if any.
    #[serde(default)]
    pub origin: Option<GraphId>,
}

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
