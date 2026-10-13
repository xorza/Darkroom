//! [`Subscription`]: one event-subscription edge.

use ::serde::{Deserialize, Serialize};

use crate::graph::identity::NodeId;

/// One event-subscription edge: `subscriber` fires when `emitter`'s event
/// `event_idx` triggers. Ordered (emitter, `event_idx`, subscriber) so the
/// `BTreeSet` holding these dedups and iterates deterministically — which is
/// what lets a compile wire each emitter's subscriber lists in a fixed order.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subscription {
    pub emitter: NodeId,
    pub event_idx: usize,
    pub subscriber: NodeId,
}

impl Subscription {
    /// Whether this edge touches `node_id` from either end — it emits to that
    /// node, or that node emits it. [`Binding::touches`](crate::graph::binding::Binding::touches) for
    /// an event edge.
    pub(crate) fn touches(&self, node_id: NodeId) -> bool {
        self.emitter == node_id || self.subscriber == node_id
    }
}
