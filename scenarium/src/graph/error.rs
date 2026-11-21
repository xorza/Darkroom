//! What an authoring graph rejects: what
//! [`validate`](crate::graph::Graph::validate) refuses to call a graph, what
//! a library cannot resolve, and the wiring
//! [`admit`](crate::graph::Graph::admit) refuses.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::graph::graph_set::ProgramShape;
use crate::graph::identity::{EventPort, GraphId, InputPort, NodeId, OutputPort};
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
    #[error("subscription {subscription:?} does not touch the detached node")]
    ForeignSubscription { subscription: Subscription },
    #[error("a ghost in the record does not touch the detached node")]
    ForeignGhost,
    #[error("input {port:?} holds a binding and a ghost")]
    GhostOnBoundInput { port: InputPort },
    #[error("subscription {subscription:?} is made and kept as a ghost")]
    GhostOfSubscription { subscription: Subscription },
}

/// Why a node declares nothing in its graph: what [`Scope::resolve`](crate::Scope::resolve)
/// answers.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unresolved {
    #[error("the library holds no func under its id")]
    MissingFunc,
    #[error("the document holds no compound under its id")]
    MissingCompound,
    #[error("a boundary node lies outside a compound, which alone has an interface")]
    NoInterface,
}

/// Why a load took a node out of its graph: it declares nothing there, or it declares something
/// a rule across the document's graphs refuses. A node detached for such a rule resolves, so the
/// reason cannot be read off the node again; the [`GhostNode`](crate::GhostNode) a detach leaves
/// keeps it.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetachReason {
    #[error(transparent)]
    Unresolved(#[from] Unresolved),
    #[error("its compound already holds an Outputs node")]
    ExtraOutputs,
    #[error("it places a compound inside itself")]
    Recursive,
    #[error("it nests compounds past {max} levels", max = ProgramId::MAX_LEN)]
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

/// Why a graph cannot hold a binding now: what [`Graph::admit_bind`](crate::Graph::admit_bind)
/// answers, for a new binding and a ghost's alike. Asked again at each load and for each Restore,
/// never stored.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum BindRejection {
    #[error("{node_id:?} is a ghost node")]
    GhostNode { node_id: NodeId },
    #[error(transparent)]
    Binding(#[from] BindingRejection),
    #[error("the wire would close a data cycle")]
    Cycle,
}

/// Why a graph cannot hold a ghost subscription now: what
/// [`Graph::admit_ghost_subscription`](crate::Graph::admit_ghost_subscription) answers.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum GhostSubscriptionRejection {
    #[error("{node_id:?} is a ghost node")]
    GhostNode { node_id: NodeId },
    #[error(transparent)]
    Subscription(#[from] SubscriptionRejection),
}

/// Why [`Graph::admit_subscription`](crate::Graph::admit_subscription)
/// refuses a subscription.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionRejection {
    #[error("the emitter does not declare the event")]
    UndeclaredEvent,
    #[error("a node cannot subscribe to its own event")]
    SelfSubscription,
    /// No node of the subscriber's kind has the trigger: the `Inputs` node has none, and a node
    /// other than the `Outputs` node has only its own, under
    /// [`TriggerPort::NODE`](crate::TriggerPort::NODE).
    #[error("no node of the subscriber's kind has that trigger")]
    ImpossibleTrigger,
    /// The subscriber shows no trigger under that id: a func that is no sink, an instance whose
    /// interface declares no trigger, an `Outputs` node whose interface declares no such event.
    #[error("the subscriber has no such trigger")]
    UndeclaredTrigger,
}

/// A graph that is not one a host may compile: its structure is broken, or it
/// breaks the load contract against a library.
#[derive(Debug, Error)]
pub enum GraphValidationError {
    #[error("graph holds a node or a ghost node under the nil id")]
    NilNodeId,
    #[error("a ghost node holds the id of node {node_id:?}")]
    GhostHoldsNode { node_id: NodeId },
    #[error("a ghost names {node_id:?}, which the graph holds as neither a node nor a ghost node")]
    GhostMissingNode { node_id: NodeId },
    #[error("input {port:?} holds a binding and a ghost")]
    GhostOnBoundInput { port: InputPort },
    #[error("subscription {subscription:?} is made and kept as a ghost")]
    GhostOfSubscription { subscription: Subscription },
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
    #[error("the compound holds no Inputs node")]
    NoInputsNode,
    #[error("the compound holds {count} Outputs nodes rather than one")]
    OutputsCount { count: usize },
    #[error("instance {node_id:?} places a compound inside itself")]
    Recursive { node_id: NodeId },
    /// The graph is past a bound of the program it expands into, as an edit refuses to make it.
    #[error(transparent)]
    Placement(#[from] PlacementRejection),
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
    #[error("it places a compound inside itself")]
    Recursive,
    #[error("graph {graph:?} would nest {levels} levels, past {max}", max = ProgramShape::MAX_LEVELS)]
    TooDeep { graph: GraphId, levels: usize },
    #[error(
        "graph {graph:?} would expand into {node_count} program nodes, past {max}",
        max = ProgramShape::MAX_NODES
    )]
    TooLarge { graph: GraphId, node_count: usize },
}
