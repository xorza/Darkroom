//! What an authoring graph rejects: what
//! [`validate`](crate::graph::Graph::validate) refuses to call a graph, what
//! a library cannot resolve, and the wiring
//! [`admit`](crate::graph::Graph::admit) refuses.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::graph::graph_set::ProgramShape;
use crate::graph::identity::{EventPort, GraphId, InputPort, NodeId, OutputPort};
use crate::graph::node::Boundary;
use crate::graph::program_id::ProgramId;
use crate::graph::subscription::Subscription;

/// Every graph validation returns this.
pub(crate) type ValidationResult<T> = Result<T, GraphValidationError>;

/// Why a [`DetachedNode`](crate::DetachedNode) record was refused.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum DetachedNodeError {
    #[error("a detached node's id must not be nil")]
    NilNodeId,
    #[error("binding on {port:?} does not touch the detached node")]
    ForeignBinding { port: InputPort },
    #[error("input {port:?} is bound twice")]
    DuplicateBinding { port: InputPort },
    #[error("subscription {subscription:?} does not touch the detached node")]
    ForeignSubscription { subscription: Subscription },
    #[error("subscription {subscription:?} appears twice")]
    DuplicateSubscription { subscription: Subscription },
}

/// Why a node declares nothing in its graph: what [`Scope::resolve`](crate::Scope::resolve)
/// answers.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unresolved {
    #[error("the library holds no func under its id")]
    MissingFunc,
    #[error("the document holds no subgraph under its id")]
    MissingSubgraph,
    #[error("a boundary node lies outside a subgraph, which alone has an interface")]
    NoInterface,
}

/// Why a load took a node out of its graph: it declares nothing there, or it declares something
/// a rule across the document's graphs refuses. A node detached for such a rule resolves, so the
/// reason cannot be read off the node again; the [`Stub`](crate::Stub) a detach leaves keeps it.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetachReason {
    #[error(transparent)]
    Unresolved(#[from] Unresolved),
    #[error("its subgraph already holds a boundary node on that side")]
    ExtraBoundary,
    #[error("it places a subgraph inside itself")]
    Recursive,
    #[error("it nests subgraphs past {max} levels", max = ProgramId::MAX_LEN)]
    TooDeep,
    #[error("it makes the program larger than {max} nodes", max = ProgramShape::MAX_NODES)]
    TooLarge,
}

/// Why [`Graph::admit`](crate::Graph::admit) refuses a binding.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum BindingRejection {
    #[error("its node does not declare the input")]
    UndeclaredInput,
    #[error("the producer does not declare the output")]
    UndeclaredOutput,
    #[error("the input is const-only and cannot take a wire")]
    WiredConstOnly,
    #[error("the input does not accept the constant")]
    ConstMismatch,
    #[error("the input does not accept the producer's output type")]
    WireMismatch,
}

/// Why [`Graph::admit_subscription`](crate::Graph::admit_subscription)
/// refuses a subscription.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionRejection {
    #[error("the emitter does not declare the event")]
    UndeclaredEvent,
    #[error("a node cannot subscribe to its own event")]
    SelfSubscription,
    #[error("the subscriber has no body to run: it is an instance or a boundary node")]
    BodilessSubscriber,
}

/// A graph that is not one a host may compile: its structure is broken, or it
/// breaks the load contract against a library.
#[derive(Debug, Error)]
pub enum GraphValidationError {
    #[error("graph contains a node with a nil id")]
    NilNodeId,
    #[error("a stub holds the id of node {node_id:?}")]
    StubHoldsNode { node_id: NodeId },
    #[error("node {node_id:?} does not resolve: {reason}")]
    Unresolved { node_id: NodeId, reason: Unresolved },
    #[error("binding on missing node {node_id:?}")]
    BindingMissingNode { node_id: NodeId },
    #[error(
        "node {destination_id:?} input {port:?} binds to missing node {source_id:?}",
        destination_id = .destination.node_id,
        port = .destination.port,
        source_id = .producer.node_id
    )]
    BindingMissingProducer {
        destination: InputPort,
        producer: OutputPort,
    },
    #[error("subscription from missing emitter {event:?}")]
    MissingSubscriptionEmitter { event: EventPort },
    #[error("event {event:?} has missing subscriber {subscriber:?}")]
    MissingSubscriber {
        event: EventPort,
        subscriber: NodeId,
    },
    #[error("the binding on {port:?} is refused: {rejection}")]
    RejectedBinding {
        port: InputPort,
        rejection: BindingRejection,
    },
    #[error("subscription {subscription:?} is refused: {rejection}")]
    RejectedSubscription {
        subscription: Subscription,
        rejection: SubscriptionRejection,
    },
    #[error("the wire into {port:?} closes a data cycle")]
    Cycle { port: InputPort },
    #[error("the subgraph holds {count} {boundary:?} nodes rather than one")]
    BoundaryCount { boundary: Boundary, count: usize },
    #[error("instance {node_id:?} places a subgraph inside itself")]
    Recursive { node_id: NodeId },
    #[error("the graph nests {levels} levels, past {max}", max = ProgramId::MAX_LEN)]
    TooDeep { levels: usize },
    #[error(
        "the graph expands into {node_count} program nodes, past {max}",
        max = ProgramShape::MAX_NODES
    )]
    TooLarge { node_count: usize },
}

/// A set of graphs that is not one a host may compile: one of its graphs breaks the load
/// contract.
#[derive(Debug, Error)]
#[error("graph {graph:?} breaks the load contract")]
pub struct GraphSetValidationError {
    pub graph: GraphId,
    #[source]
    pub error: GraphValidationError,
}

/// Why [`GraphSet::admit_nodes`](crate::GraphSet::admit_nodes) refuses to add nodes to a graph.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PlacementRejection {
    #[error("it places a subgraph inside itself")]
    Recursive,
    #[error("graph {graph:?} would nest {levels} levels, past {max}", max = ProgramShape::MAX_LEVELS)]
    TooDeep { graph: GraphId, levels: usize },
    #[error(
        "graph {graph:?} would expand into {node_count} program nodes, past {max}",
        max = ProgramShape::MAX_NODES
    )]
    TooLarge { graph: GraphId, node_count: usize },
}
