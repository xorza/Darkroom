//! Why an intent could never have applied, and why the document refuses one that could.

use crate::core::document::compound_side::ReadOnly;
use std::fmt;
use std::fmt::{Display, Formatter};

use glam::Vec2;
use scenarium::{
    BindingRejection, DetachedNodeError, InputPort, NodeId, PlacementRejection, Subscription,
    SubscriptionRejection, Unresolved,
};
use thiserror::Error;

use crate::core::document::tie::TieId;
use crate::core::document::wire_key::WireKey;

/// Why an intent wrote nothing: its payload could never have applied, or the document's rules
/// refuse it.
#[derive(Debug, Error, Clone, Copy, PartialEq)]
pub(crate) enum IntentError {
    #[error("a widget built a malformed intent")]
    Malformed(#[from] MalformedIntent),
    #[error("the edit was refused")]
    Refused(#[from] EditRefusal),
}

/// An edit a person can ask for that the document's rules refuse: it writes nothing, and the
/// status bar names why. Refused by design as a cycle-forming bind is, but worth a line, as no
/// widget can hide the ask before it is made.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditRefusal {
    /// A node whose placement [`scenarium::GraphSet::admit_nodes`] refuses: an instance of a
    /// compound inside itself, or one past a bound.
    #[error("the node was not added")]
    Placement(#[source] PlacementRejection),
    /// A removal of a compound's `Outputs` node or its last `Inputs` node, or a second
    /// `Outputs` node added.
    #[error("a compound keeps one Outputs node and at least one Inputs node")]
    BoundaryNode,
    /// A removal of a compound that nodes still place.
    #[error("the compound is placed by {uses} node(s); remove them first")]
    CompoundInUse { uses: usize },
    /// An edit of the graph or interface of a compound that follows the library or is locked.
    #[error("{}", match why {
        ReadOnly::Follows => "the compound follows the library; Make Local to edit it",
        ReadOnly::Locked => "the compound is locked; Unlock it to edit it",
    })]
    ReadOnly { why: ReadOnly },
}

/// A payload no widget could legitimately have raised: a nil or colliding
/// identity, a non-finite position, a link to state the document doesn't hold.
///
/// **This is our own bug, not a refusal.** Widgets read every identity they emit out of the live
/// document, so the worst they manage is *stale*, and a stale intent is not an error: it yields
/// no step, which [`GraphIntent::commit`](crate::core::edit::graph_intent::GraphIntent::commit)
/// drops silently, as it does a no-op and a cycle-forming bind. Only the cases below are an
/// `Err`.
#[derive(Debug, Error, Clone, Copy, PartialEq)]
pub(crate) enum MalformedIntent {
    /// A nil id where the intent must name something: never stale, as no
    /// document holds it.
    #[error("{role} node id is nil")]
    NilNodeId { role: Role },
    /// A nil tie id, which no view holds, as [`Self::NilNodeId`] is for a node.
    #[error("{role} tie id is nil")]
    NilTieId { role: Role },
    /// A new id the document already holds. Scenarium requires node ids unique inside a graph,
    /// and `Graph::insert` panics on a collision.
    #[error("node {node_id:?} already exists in the document")]
    NodeAlreadyExists { node_id: NodeId },
    /// An inserted node the library cannot resolve: the graph holds only nodes
    /// it can run.
    #[error("new node does not resolve: {reason}")]
    UnresolvedNode { reason: Unresolved },
    /// A cache mode set on a node with no body, an instance or a boundary node, which caches
    /// nothing of its own: the canvas draws no cache chips on one, so this is a widget's bug.
    #[error("node {node_id:?} has no body, so no cache mode")]
    NoCacheMode { node_id: NodeId },
    /// A compound set on a node that is no instance, or one whose interface is not the
    /// instance's own: a re-pointing keeps every port, so a widget offers neither.
    #[error("node {node_id:?} cannot be pointed at that compound")]
    NotRepointable { node_id: NodeId },
    /// An insertion's own wiring pointing at a node the graph doesn't hold. Unlike a stale
    /// reference this is malformed: the wiring is authored together with its node.
    #[error("{role} node {node_id:?} is not in the graph")]
    NodeAbsent { role: Role, node_id: NodeId },
    /// A seed binding reading the very node it feeds: a cycle of one, which
    /// every compile refuses with a panic.
    #[error("seed binding on {port:?} reads the node it is being added to")]
    CyclicSeedBinding { port: InputPort },
    /// A binding the graph may not hold ([`scenarium::Graph::admit`]). The
    /// canvas offers only bindings that fit, so one here is a widget's bug.
    #[error("the binding on {port:?} is refused: {rejection}")]
    Rejected {
        port: InputPort,
        rejection: BindingRejection,
    },
    /// A subscription the graph may not hold
    /// ([`scenarium::Graph::admit_subscription`]).
    #[error("subscription {subscription:?} is refused: {rejection}")]
    RejectedSubscription {
        subscription: Subscription,
        rejection: SubscriptionRejection,
    },
    /// A seed binding landing somewhere other than the node being inserted: an insertion may only
    /// author its own node's inputs, anything else would edit a node the step does not carry.
    #[error("seed binding on {port:?} does not belong to the inserted node")]
    ForeignSeedBinding { port: InputPort },
    /// The insertion's record is not one a graph could hold, as with a port seeded twice.
    #[error("the inserted node's record is malformed")]
    InsertionRecord(#[from] DetachedNodeError),
    /// Positions reach the view verbatim, and nothing after the load repairs a
    /// non-finite one.
    #[error("{role} position {pos:?} is not finite")]
    NonFinitePos { role: Role, pos: Vec2 },
    /// Same corruption path as [`Self::NonFinitePos`], one level up.
    #[error("viewport needs finite pan and positive finite zoom")]
    InvalidViewport,
    /// A new tie under an id the view already holds.
    #[error("tie {tie:?} already exists in the view")]
    TieAlreadyExists { tie: TieId },
    /// A new tie that holds no wire, which no view may hold.
    #[error("new tie {tie:?} holds no wire")]
    EmptyTie { tie: TieId },
    /// A tie line reads the wires it crosses out of the live document, so a wire the graph does
    /// not hold is malformed.
    #[error("wire {wire:?} is not in the graph")]
    WireAbsent { wire: WireKey },
    /// A crossed piece past the wire's last one.
    #[error("piece {at} of wire {wire:?} is past its route of {len} tie(s)")]
    RouteIndexOutOfRange {
        wire: WireKey,
        at: usize,
        len: usize,
    },
    /// A tie a wire already runs through: a route names a tie once.
    #[error("wire {wire:?} already runs through tie {tie:?}")]
    TieAlreadyOnRoute { wire: WireKey, tie: TieId },
    /// A pinned angle outside `[0, 360)`, which the view refuses.
    #[error("pinned angle {degrees} of tie {tie:?} is not in [0, 360)")]
    InvalidTieAngle { tie: TieId, degrees: f32 },
}

/// Which identity or position of an intent a check was holding to, named by a
/// [`MalformedIntent`] alongside the value it refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// The id an insertion introduces.
    New,
    /// The producer an insertion's seed binding reads.
    BindingProducer,
    AddNode,
    RemoveNode,
    MoveSelection,
    RenameNode,
    SetInputDestination,
    SetInputProducer,
    SetNodeProperty,
    SetSubscriptionEmitter,
    SetSubscriptionSubscriber,
    Raise,
    TieWires,
    JoinTie,
    RemoveTie,
    SetTieAngle,
    ReleaseWire,
}

impl Display for Role {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::New => "new",
            Self::BindingProducer => "binding producer",
            Self::AddNode => "AddNode",
            Self::RemoveNode => "RemoveNode",
            Self::MoveSelection => "MoveSelection",
            Self::RenameNode => "RenameNode",
            Self::SetInputDestination => "SetInput destination",
            Self::SetInputProducer => "SetInput producer",
            Self::SetNodeProperty => "SetNodeProperty",
            Self::SetSubscriptionEmitter => "SetSubscription emitter",
            Self::SetSubscriptionSubscriber => "SetSubscription subscriber",
            Self::Raise => "Raise",
            Self::TieWires => "TieWires",
            Self::JoinTie => "JoinTie",
            Self::RemoveTie => "RemoveTie",
            Self::SetTieAngle => "SetTieAngle",
            Self::ReleaseWire => "ReleaseWire",
        })
    }
}
