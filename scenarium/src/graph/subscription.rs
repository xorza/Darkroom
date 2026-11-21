//! [`Subscription`]: one event-subscription edge, from an event to a trigger.

use ::serde::{Deserialize, Serialize};

use crate::identity::{EventPort, NodeId, TriggerPort};

/// One event-subscription edge: `trigger` is triggered when `event` fires, as a wire joins an
/// output to an input; the event side's
/// [`Binding::Wire`](crate::graph::binding::Binding::Wire). Ordered by event (emitter, then
/// event id), then trigger, so a compile wires each emitter's subscribers in a fixed order.
///
/// A trigger is a sink's or an instance's own, which runs the node or passes the event into the
/// compound, or the `Outputs` node's, one per interface event, which passes it out. Every trigger
/// is addressed alike, a node's own under [`TriggerPort::NODE`], as Unreal names a node's one exec
/// input the pin "execute" and Unity declares a node's one control input as a named port
/// ([custom node ports](https://docs.unity3d.com/Packages/com.unity.visualscripting@1.9/manual/vs-create-custom-node-add-ports.html)).
/// Refused: a trigger port on every func, which turns each node's one trigger into a list; an
/// address type per kind of trigger, which every site that handles a subscription's end matches
/// on.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    pub event: EventPort,
    pub trigger: TriggerPort,
}

impl Subscription {
    pub fn new(event: EventPort, trigger: impl Into<TriggerPort>) -> Self {
        Self {
            event,
            trigger: trigger.into(),
        }
    }

    /// Whether this edge touches `node_id` from either end;
    /// [`Binding::touches`](crate::graph::binding::Binding::touches) for an event edge.
    pub(crate) fn touches(&self, node_id: NodeId) -> bool {
        self.event.node_id == node_id || self.trigger.node_id == node_id
    }
}
