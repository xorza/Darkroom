use crate::execution::identity::{ExecutionEventPort, ExecutionNodeId};

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
    pub events: Vec<ExecutionEventPort>,
    /// Run these exact compiled nodes and deliver every output — the on-demand "run to
    /// this node" / preview trigger. An explicitly seeded disabled node is enabled for
    /// this run; an identity absent from the installed program fails with
    /// [`Error::NodeSeedNotFound`](crate::execution::error::Error::NodeSeedNotFound).
    pub nodes: Vec<ExecutionNodeId>,
}

impl RunSeeds {
    pub fn sinks() -> Self {
        Self {
            sinks: true,
            ..Self::default()
        }
    }

    pub fn nodes(nodes: Vec<ExecutionNodeId>) -> Self {
        Self {
            nodes,
            ..Self::default()
        }
    }

    pub fn events(events: Vec<ExecutionEventPort>) -> Self {
        Self {
            events,
            ..Self::default()
        }
    }
}
