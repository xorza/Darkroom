//! [`Subscription`]: one event-subscription edge.

use ::serde::{Deserialize, Serialize};

use crate::graph::identity::{EventPort, NodeId};

/// One event-subscription edge: `subscriber` fires when `event` triggers — the
/// event side's [`Binding::Wire`](crate::graph::binding::Binding::Wire). Ordered
/// by event (emitter, then event index), then subscriber, so the `BTreeSet`
/// holding these dedups and iterates deterministically — which is what lets a
/// compile wire each emitter's subscriber lists in a fixed order.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subscription {
    pub event: EventPort,
    pub subscriber: NodeId,
}

impl Subscription {
    pub const fn new(event: EventPort, subscriber: NodeId) -> Self {
        Self { event, subscriber }
    }

    /// Whether this edge touches `node_id` from either end — it emits to that
    /// node, or that node emits it. [`Binding::touches`](crate::graph::binding::Binding::touches) for
    /// an event edge.
    pub(crate) fn touches(&self, node_id: NodeId) -> bool {
        self.event.node_id == node_id || self.subscriber == node_id
    }
}
