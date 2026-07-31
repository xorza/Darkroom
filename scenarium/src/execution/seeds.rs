use crate::graph::identity::{EventPort, NodeId};

/// What seeds a run's schedule — the roots the planner walks back from. The four
/// are independent and combine: a run can target sink nodes, the event loop's
/// triggerable events, a set of injected events, and/or specific nodes, all at once.
#[derive(Debug, Default, Clone)]
pub struct RunSeeds {
    /// Include all sink nodes — the ordinary "produce the outputs" trigger.
    pub sinks: bool,
    /// Include every node owning a subscribed event so it initializes the
    /// shared state and runtime triggers that drive the event loop.
    pub event_sources: bool,
    /// Run the subscribers of these specific fired events. An event absent from the
    /// installed program fails with
    /// [`Error::EventSeedNotFound`](crate::execution::error::Error::EventSeedNotFound).
    pub events: Vec<EventPort>,
    /// Run these exact compiled nodes and deliver every output — the on-demand "run to
    /// this node" / preview trigger. An explicitly seeded disabled node is enabled for
    /// this run; an identity absent from the installed program fails with
    /// [`Error::NodeSeedNotFound`](crate::execution::error::Error::NodeSeedNotFound).
    pub node_ids: Vec<NodeId>,
}

impl RunSeeds {
    pub fn sinks() -> Self {
        Self {
            sinks: true,
            ..Self::default()
        }
    }

    pub fn nodes(node_ids: Vec<NodeId>) -> Self {
        Self {
            node_ids,
            ..Self::default()
        }
    }

    pub fn events(events: Vec<EventPort>) -> Self {
        Self {
            events,
            ..Self::default()
        }
    }

    /// Whether these seeds would start no run at all — every trigger off and
    /// both lists empty.
    pub(crate) fn is_empty(&self) -> bool {
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

    /// Fold `other` in: the triggers are independent, so they *or* together,
    /// and the two lists gain what they don't already name.
    ///
    /// This is what lets a worker coalesce a burst of `Run` messages into one
    /// run. It lives here rather than on the batch that calls it because the
    /// combining rule is a property of the seeds — "these four combine" is the
    /// same sentence the type's own doc opens with — and a batch that spelled
    /// its own four fields had to restate it.
    pub(crate) fn merge(&mut self, other: RunSeeds) {
        let RunSeeds {
            sinks,
            event_sources,
            events,
            node_ids,
        } = other;
        self.sinks |= sinks;
        self.event_sources |= event_sources;
        self.add_events(events);
        extend_unique(&mut self.node_ids, node_ids);
    }

    /// Seed the subscribers of `events` too — the event loop's fired events,
    /// which reach a batch outside any `Run` message.
    pub(crate) fn add_events(&mut self, events: impl IntoIterator<Item = EventPort>) {
        extend_unique(&mut self.events, events);
    }
}

/// Append what `incoming` adds, in first-seen order, dropping what is already
/// there.
///
/// A linear scan rather than a hash set: one batch is one burst of worker
/// messages, so these lists hold a handful of ids — building a set costs more
/// than the scan it would save, and the run's seeds keep the order the host
/// asked in.
fn extend_unique<T: PartialEq>(into: &mut Vec<T>, incoming: impl IntoIterator<Item = T>) {
    for item in incoming {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}
