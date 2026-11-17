//! [`Reconciled`]: what [`GraphSet::reconcile`](crate::GraphSet::reconcile) changed in each
//! decoded graph so that it holds only what its scope can run.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Display, Formatter};

use crate::data::const_value::ConstValue;
use crate::graph::binding::Binding;
use crate::graph::error::{
    DetachReason, GhostRejection, GhostSubscriptionRejection, SubscriptionRejection,
};
use crate::graph::identity::{CompoundId, FuncId, InputPort, NodeId};
use crate::graph::node::Boundary;
use crate::graph::subscription::Subscription;

/// Every change a reconcile made to one graph, in the order it made them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    pub repairs: Vec<Repair>,
}

impl Reconciled {
    /// Whether the graph was already one the library can run.
    pub const fn is_empty(&self) -> bool {
        self.repairs.is_empty()
    }
}

/// Every change a load made to a set of graphs, graph by graph: what
/// [`GraphSet::reconcile`](crate::GraphSet::reconcile) reports.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GraphSetReconciled {
    pub main: Reconciled,
    /// Only the compounds a load changed.
    pub compounds: BTreeMap<CompoundId, Reconciled>,
}

impl GraphSetReconciled {
    /// Whether the set was already one the library can run.
    pub fn is_empty(&self) -> bool {
        self.main.is_empty() && self.compounds.is_empty()
    }
}

/// One thing a reconcile changed, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repair {
    /// A node under the nil id, which no lookup can address.
    NilNode,
    /// A ghost node saved under the id of a node, which the graph keeps.
    DuplicateGhostNode { node_id: NodeId },
    /// A ghost node that resolves again, attached to the graph.
    Reattached { node_id: NodeId },
    /// A node or a ghost node of a func the library no longer registers, moved with its wiring
    /// to the func its redirects end at.
    Redirected {
        node_id: NodeId,
        from: FuncId,
        to: FuncId,
    },
    /// A node taken out of the graph and kept as a ghost node, its wiring as ghosts.
    GhostedNode {
        node_id: NodeId,
        reason: DetachReason,
    },
    /// A binding, live or ghost, whose consumer or producer the graph holds as neither a node
    /// nor a ghost node.
    DanglingBinding { port: InputPort, binding: Binding },
    /// A subscription, live or ghost, whose emitter or subscriber the graph holds as neither.
    DanglingSubscription { subscription: Subscription },
    /// A ghost saved on an input the graph has bound, which the binding replaces.
    GhostOnBoundInput { port: InputPort, binding: Binding },
    /// A ghost saved of a subscription the graph has made.
    GhostOfSubscription { subscription: Subscription },
    /// A binding the graph cannot hold, kept as a ghost.
    GhostedBinding {
        port: InputPort,
        binding: Binding,
        rejection: GhostRejection,
    },
    /// A subscription the graph cannot hold, kept as a ghost.
    GhostedSubscription {
        subscription: Subscription,
        rejection: GhostSubscriptionRejection,
    },
    /// A ghost binding the graph can hold again, made live.
    RestoredBinding { port: InputPort, binding: Binding },
    /// A ghost subscription the graph can hold again, made live.
    RestoredSubscription { subscription: Subscription },
    /// A ghost constant that was the default its port declared, which nobody set: removed.
    DefaultDropped { port: InputPort, value: ConstValue },
    /// An input its func gained, or whose default went as a ghost, bound to the default it
    /// declares, as on a new node.
    DefaultSeeded { port: InputPort, binding: Binding },
    /// A subscription no library can let a graph hold, live or as a ghost: a node's to its own
    /// event, or one on a trigger no node of its subscriber's kind has.
    RejectedSubscription {
        subscription: Subscription,
        rejection: SubscriptionRejection,
    },
    /// A compound's graph lacked the boundary node on one side, which this one now is.
    BoundaryAdded { boundary: Boundary, node_id: NodeId },
}

impl Display for Repair {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NilNode => f.write_str("removed a node with the nil id"),
            Self::DuplicateGhostNode { node_id } => {
                write!(
                    f,
                    "removed a ghost node of {node_id:?}, which the graph holds"
                )
            }
            Self::Reattached { node_id } => {
                write!(
                    f,
                    "attached the ghost node {node_id:?} again: its node resolves"
                )
            }
            Self::Redirected { node_id, from, to } => {
                write!(
                    f,
                    "moved {node_id:?} from func {from:?}, which the library redirects, to {to:?}"
                )
            }
            Self::GhostedNode { node_id, reason } => {
                write!(f, "kept {node_id:?} as a ghost node: {reason}")
            }
            Self::DanglingBinding { port, binding } => {
                write!(
                    f,
                    "removed {binding:?} on {port:?}, which names a missing node"
                )
            }
            Self::DanglingSubscription { subscription } => {
                write!(f, "removed {subscription:?}, which names a missing node")
            }
            Self::GhostOnBoundInput { port, binding } => {
                write!(
                    f,
                    "removed the ghost {binding:?} on {port:?}: the input is bound"
                )
            }
            Self::GhostOfSubscription { subscription } => {
                write!(
                    f,
                    "removed the ghost of {subscription:?}: the subscription is made"
                )
            }
            Self::GhostedBinding {
                port,
                binding,
                rejection,
            } => write!(f, "kept {binding:?} on {port:?} as a ghost: {rejection}"),
            Self::GhostedSubscription {
                subscription,
                rejection,
            } => write!(f, "kept {subscription:?} as a ghost: {rejection}"),
            Self::RestoredBinding { port, binding } => {
                write!(
                    f,
                    "restored the ghost {binding:?} on {port:?}: the graph holds it again"
                )
            }
            Self::RestoredSubscription { subscription } => {
                write!(
                    f,
                    "restored the ghost of {subscription:?}: the graph holds it again"
                )
            }
            Self::DefaultDropped { port, value } => {
                write!(
                    f,
                    "removed {value:?} on {port:?}: it was the default of a port that changed"
                )
            }
            Self::DefaultSeeded { port, binding } => {
                write!(f, "bound {port:?} to its default {binding:?}")
            }
            Self::RejectedSubscription {
                subscription,
                rejection,
            } => write!(f, "removed {subscription:?}: {rejection}"),
            Self::BoundaryAdded { boundary, node_id } => {
                write!(f, "added the missing {boundary:?} node as {node_id:?}")
            }
        }
    }
}
