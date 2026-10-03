use crate::containers::unique;
use crate::graph::identity::{EventPort, NodeId};

/// What seeds a run's schedule — the roots the planner walks back from. The four
/// are independent and combine: a run can target sink nodes, the event loop's
/// event sources, fired events, and/or specific nodes, all at once.
///
/// The worker's own: a host asks for sinks or nodes through
/// [`WorkerMessage`](crate::WorkerMessage), and the worker adds the events its
/// loop fires and the sources a loop start initializes.
#[derive(Debug, Default, Clone)]
pub(crate) struct RunSeeds {
    /// Include all sink nodes — the ordinary "produce the outputs" trigger.
    pub(crate) sinks: bool,
    /// Include every node owning a subscribed event so it initializes the
    /// shared state and runtime triggers that drive the event loop.
    pub(crate) event_sources: bool,
    /// Run the subscribers of these specific fired events. An event absent from the
    /// installed program fails with
    /// [`Error::EventSeedNotFound`](crate::execution::error::Error::EventSeedNotFound).
    pub(crate) events: Vec<EventPort>,
    /// Run these exact compiled nodes and deliver every output — the on-demand "run to
    /// this node" / preview trigger. An explicitly seeded disabled node is enabled for
    /// this run; an identity absent from the installed program fails with
    /// [`Error::NodeSeedNotFound`](crate::execution::error::Error::NodeSeedNotFound).
    pub(crate) node_ids: Vec<NodeId>,
}

impl RunSeeds {
    /// Whether these seeds would start no run at all — every trigger off and
    /// both lists empty.
    pub(crate) const fn is_empty(&self) -> bool {
        !self.sinks && !self.event_sources && self.events.is_empty() && self.node_ids.is_empty()
    }

    /// Forget everything, keeping the lists' capacity. What a worker batch
    /// opens with, so one burst's seeds never leak into the next.
    pub(crate) fn clear(&mut self) {
        self.sinks = false;
        self.event_sources = false;
        self.events.clear();
        self.node_ids.clear();
    }

    /// Seed the subscribers of `events` too — the event loop's fired events,
    /// which reach a batch outside any `Run` message.
    pub(crate) fn add_events(&mut self, events: impl IntoIterator<Item = EventPort>) {
        unique::extend(&mut self.events, events);
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::seeds::RunSeeds;
    use crate::graph::identity::{EventPort, NodeId};

    impl RunSeeds {
        pub(crate) fn sinks() -> Self {
            Self {
                sinks: true,
                ..Self::default()
            }
        }

        pub(crate) fn nodes(node_ids: Vec<NodeId>) -> Self {
            Self {
                node_ids,
                ..Self::default()
            }
        }

        pub(crate) fn events(events: Vec<EventPort>) -> Self {
            Self {
                events,
                ..Self::default()
            }
        }
    }
}
