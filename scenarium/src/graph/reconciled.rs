//! [`Reconciled`]: what [`Graph::reconcile`](crate::Graph::reconcile) changed in a decoded graph
//! so that it holds only what a library can run.

use std::fmt;
use std::fmt::{Display, Formatter};

use crate::graph::binding::Binding;
use crate::graph::binding_entry::BindingEntry;
use crate::graph::error::{BindingRejection, DetachReason, SubscriptionRejection};
use crate::graph::identity::{InputPort, NodeId, OutputPort};
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

/// One thing a reconcile changed, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repair {
    /// A node under the nil id, which no lookup can address.
    NilNode,
    /// A stub saved under the id of a node, which the graph keeps.
    DuplicateStub { node_id: NodeId },
    /// A stub whose node resolves again, attached to the graph.
    Reattached { node_id: NodeId },
    /// A reattached stub's binding whose other end is gone, or whose input the graph has bound
    /// since.
    StubBindingDropped {
        node_id: NodeId,
        entry: BindingEntry,
    },
    /// A reattached stub's subscription whose other end is gone.
    StubSubscriptionDropped {
        node_id: NodeId,
        subscription: Subscription,
    },
    /// A node taken out of the graph and kept as a stub.
    Stubbed {
        node_id: NodeId,
        reason: DetachReason,
    },
    /// A binding whose consumer or producer the graph does not hold.
    DanglingBinding { port: InputPort, binding: Binding },
    /// A subscription whose emitter or subscriber the graph does not hold.
    DanglingSubscription { subscription: Subscription },
    RejectedBinding {
        port: InputPort,
        binding: Binding,
        rejection: BindingRejection,
    },
    RejectedSubscription {
        subscription: Subscription,
        rejection: SubscriptionRejection,
    },
    /// A wire that closed a data cycle.
    CycleWire {
        port: InputPort,
        producer: OutputPort,
    },
}

impl Display for Repair {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NilNode => f.write_str("removed a node with the nil id"),
            Self::DuplicateStub { node_id } => {
                write!(f, "removed a stub of {node_id:?}, which the graph holds")
            }
            Self::Reattached { node_id } => {
                write!(f, "attached the stub {node_id:?} again: its node resolves")
            }
            Self::StubBindingDropped { node_id, entry } => write!(
                f,
                "removed {:?} on {:?} from the stub {node_id:?}: its other end is gone, or the \
                 input is bound",
                entry.binding, entry.port
            ),
            Self::StubSubscriptionDropped {
                node_id,
                subscription,
            } => write!(
                f,
                "removed {subscription:?} from the stub {node_id:?}: its other end is gone"
            ),
            Self::Stubbed { node_id, reason } => {
                write!(f, "kept {node_id:?} as a stub: {reason}")
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
            Self::RejectedBinding {
                port,
                binding,
                rejection,
            } => write!(f, "removed {binding:?} on {port:?}: {rejection}"),
            Self::RejectedSubscription {
                subscription,
                rejection,
            } => write!(f, "removed {subscription:?}: {rejection}"),
            Self::CycleWire { port, producer } => {
                write!(
                    f,
                    "removed the wire {producer:?} → {port:?}, which closed a data cycle"
                )
            }
        }
    }
}
