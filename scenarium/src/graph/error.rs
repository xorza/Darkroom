//! What an authoring graph rejects: what
//! [`validate`](crate::graph::Graph::validate) refuses to call a graph, what
//! a library cannot resolve, and the wiring
//! [`admit`](crate::graph::Graph::admit) refuses.

use thiserror::Error;

use crate::graph::identity::NodeId;
use crate::graph::identity::{EventPort, InputPort, OutputPort};
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

/// Why a node has no declaration in a library: what
/// [`Library::func_of`](crate::Library::func_of) answers for a node a graph
/// cannot hold.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum Unresolved {
    #[error("the library holds no func under its id")]
    MissingFunc,
    #[error("the library holds its func with other ports than it was authored against")]
    ChangedPorts,
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
}

/// A graph that is not one a host may compile: its structure is broken, or it
/// breaks the load contract against a library.
#[derive(Debug, Error)]
pub enum GraphValidationError {
    #[error("graph contains a node with a nil id")]
    NilNodeId,
    #[error("node {node_id:?} does not resolve: {reason}")]
    Unresolved { node_id: NodeId, reason: Unresolved },
    #[error("binding on missing node {node_id:?}")]
    BindingMissingNode { node_id: NodeId },
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
}
