//! [`WireKey`]: one wire of the graph, named by both of its ends.

use scenarium::{
    Graph as CoreGraph, InputPort, NodeId, OutputPort, Subscription, SubscriptionState,
};
use serde::{Deserialize, Serialize};

/// One wire, data or event, by both of its ends: rewiring either end makes another key, so a route
/// kept for the old wire names no wire the graph holds. `darkroom/AGENTS.md`, "Wire ties", "A wire
/// is both of its ends", holds the decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum WireKey {
    Data(DataWire),
    Event(Subscription),
}

/// A data wire: `input` bound to `output`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct DataWire {
    pub(crate) output: OutputPort,
    pub(crate) input: InputPort,
}

impl WireKey {
    /// The nodes at the wire's two ends, the output's or the emitter's first.
    pub(crate) const fn ends(self) -> [NodeId; 2] {
        match self {
            Self::Data(DataWire { output, input }) => [output.node_id, input.node_id],
            Self::Event(subscription) => {
                [subscription.event.node_id, subscription.listener.node_id()]
            }
        }
    }

    /// Whether `graph` holds this wire, live or as a ghost: the input is wired to this output,
    /// or the subscription is made or kept. A wire is the same wire while it is a ghost, so its
    /// route outlives a load that cannot hold it.
    pub(crate) fn is_held_by(self, graph: &CoreGraph) -> bool {
        match self {
            Self::Data(DataWire { output, input }) => match graph.input_state_wire(input) {
                Some(source) => source == output,
                None => false,
            },
            Self::Event(subscription) => {
                graph.subscription_state(subscription) != SubscriptionState::Absent
            }
        }
    }
}
