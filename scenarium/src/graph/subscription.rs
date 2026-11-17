//! [`Subscription`]: one event-subscription edge, and [`Listener`], the end it lands on.

use ::serde::{Deserialize, Serialize};

use crate::graph::identity::{EventPort, NodeId, TriggerPort};

/// One event-subscription edge: `listener` hears `event` fire, the event side's
/// [`Binding::Wire`](crate::graph::binding::Binding::Wire). Ordered by event (emitter, then
/// event id), then listener, so a compile wires each emitter's listeners in a fixed order.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subscription {
    pub event: EventPort,
    pub listener: Listener,
}

/// Where a subscription lands: a node's pin, which runs a func's node and passes the event into
/// an instance's subgraph, or a trigger an `Outputs` node hears, which passes it out.
///
/// Every node that hears from outside hears on its one pin: a func, and an instance, whose
/// interface has one trigger at most
/// ([`Interface`](crate::Interface)). The `Outputs` node is heard on a trigger for each event its
/// interface carries, as each passes out as an event of its own. Refused: a trigger port on every
/// func, which turns each pin into a list; a list of triggers on an instance, drawn apart from
/// every other node's pin.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Listener {
    Pin(NodeId),
    Trigger(TriggerPort),
}

impl Listener {
    /// The node the listener is on.
    pub const fn node_id(self) -> NodeId {
        match self {
            Self::Pin(node_id) => node_id,
            Self::Trigger(trigger) => trigger.node_id,
        }
    }
}

impl Listener {
    /// This listener on `node_id`, its kind and port kept: a listener of a node copied under a
    /// new id.
    #[must_use]
    pub const fn on(self, node_id: NodeId) -> Self {
        match self {
            Self::Pin(_) => Self::Pin(node_id),
            Self::Trigger(trigger) => Self::Trigger(TriggerPort::new(node_id, trigger.trigger)),
        }
    }
}

impl From<NodeId> for Listener {
    /// The pin of `node_id`.
    fn from(node_id: NodeId) -> Self {
        Self::Pin(node_id)
    }
}

impl From<TriggerPort> for Listener {
    fn from(trigger: TriggerPort) -> Self {
        Self::Trigger(trigger)
    }
}

impl Subscription {
    pub fn new(event: EventPort, listener: impl Into<Listener>) -> Self {
        Self {
            event,
            listener: listener.into(),
        }
    }

    /// Whether this edge touches `node_id` from either end;
    /// [`Binding::touches`](crate::graph::binding::Binding::touches) for an event edge.
    pub(crate) fn touches(&self, node_id: NodeId) -> bool {
        self.event.node_id == node_id || self.listener.node_id() == node_id
    }
}
