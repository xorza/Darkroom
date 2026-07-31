//! What an authoring graph rejects: everything
//! [`validate`](crate::graph::Graph::validate) refuses to call a graph.
//!
//! *Recoverable* — a stale document or an edit a user can undo — so it is a
//! `Result` rather than a panic. Logic errors inside the graph's own mutations
//! assert instead.

use thiserror::Error;

/// Every graph validation returns this.
pub(crate) type ValidationResult<T> = Result<T, GraphValidationError>;

use crate::graph::identity::FuncId;
use crate::graph::identity::NodeId;
use crate::graph::identity::{InputPort, OutputPort};

#[derive(Debug, Error)]
pub enum GraphValidationError {
    #[error("graph contains a node with a nil id")]
    NilNodeId,
    #[error("node id {node_id:?} occurs more than once")]
    DuplicateNodeId { node_id: NodeId },
    #[error("node {node_id:?} has a nil func_id")]
    NilFuncId { node_id: NodeId },
    #[error("node {node_id:?} references func {func_id:?}, absent from the library")]
    MissingFunc { node_id: NodeId, func_id: FuncId },
    #[error("binding on missing node {node_id:?}")]
    BindingMissingNode { node_id: NodeId },
    #[error(
        "input {port_idx} on node {node_id:?} is const-only and cannot be wired to an upstream output",
        node_id = .port.node_id,
        port_idx = .port.port_idx
    )]
    ConstOnlyBinding { port: InputPort },
    #[error(
        "node {destination_id:?} input {port_idx} binds to missing node {source_id:?}",
        destination_id = .destination.node_id,
        port_idx = .destination.port_idx,
        source_id = .producer.node_id
    )]
    BindingMissingProducer {
        destination: InputPort,
        producer: OutputPort,
    },
    #[error("subscription from missing emitter {node_id:?}")]
    MissingSubscriptionEmitter { node_id: NodeId },
    #[error("node {emitter:?} event {event_idx} has missing subscriber {subscriber:?}")]
    MissingSubscriber {
        emitter: NodeId,
        event_idx: usize,
        subscriber: NodeId,
    },
}
