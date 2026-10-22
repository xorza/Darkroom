//! [`Reconciled`]: what [`Graph::reconcile`](crate::Graph::reconcile) took out
//! of a decoded graph so that it holds only what a library can run.

use std::fmt;
use std::fmt::{Display, Formatter};

use crate::graph::binding::Binding;
use crate::graph::detached::DetachedNode;
use crate::graph::error::{BindingRejection, SubscriptionRejection, Unresolved};
use crate::graph::identity::{InputPort, OutputPort};
use crate::graph::subscription::Subscription;

/// Every change a reconcile made: the wiring it removed, and the nodes it
/// detached with the wiring that touched them.
#[derive(Debug, Default)]
pub struct Reconciled {
    pub repairs: Vec<Repair>,
    /// In node-id order. Each record holds the wiring that touched its node
    /// when it was detached, so a node detached later does not hold an edge
    /// an earlier record took.
    pub detached: Vec<Detachment>,
}

impl Reconciled {
    /// Whether the graph was already one the library can run.
    pub const fn is_empty(&self) -> bool {
        self.repairs.is_empty() && self.detached.is_empty()
    }
}

/// A node the library cannot resolve, taken out with its wiring.
#[derive(Debug)]
pub struct Detachment {
    pub record: DetachedNode,
    pub reason: Unresolved,
}

/// One thing a reconcile removed, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repair {
    /// A node under the nil id, which no lookup can address.
    NilNode,
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
