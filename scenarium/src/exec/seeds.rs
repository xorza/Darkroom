use crate::exec::program_event::ProgramEvent;
use crate::graph::program_id::ProgramId;

/// What seeds a run's schedule — the roots the planner walks back from. The four
/// are independent and combine: a run can target sink nodes, the event loop's
/// event sources, fired events, and/or specific nodes, all at once.
///
/// The worker's own: a host asks for sinks or nodes through
/// [`WorkerMessage`](crate::WorkerMessage), and the worker adds the events its
/// loop fires and the sources a loop start initializes.
#[derive(Debug, Default, Clone)]
pub(crate) struct RunSeeds {
    /// Include all sink nodes — the ordinary "produce the outputs" run.
    pub(crate) sinks: bool,
    /// Include every node owning a subscribed event so it initializes the
    /// shared state and arms the events that drive the event loop.
    pub(crate) event_sources: bool,
    /// Run the subscribers of these specific fired events, each one the installed program
    /// declares.
    pub(crate) events: Vec<ProgramEvent>,
    /// Run these exact compiled nodes and deliver every output — the on-demand "run to
    /// this node" / preview trigger. An explicitly seeded disabled node is enabled for
    /// this run; an identity absent from the installed program fails with
    /// [`ExecError::NodeSeedNotFound`](crate::exec::error::ExecError::NodeSeedNotFound).
    pub(crate) program_ids: Vec<ProgramId>,
}

impl RunSeeds {
    /// Whether these seeds would start no run at all — every trigger off and
    /// both lists empty.
    pub(crate) const fn is_empty(&self) -> bool {
        !self.sinks && !self.event_sources && self.events.is_empty() && self.program_ids.is_empty()
    }

    /// Forget everything, keeping the lists' capacity, so one burst's seeds never leak into the
    /// next.
    pub(crate) fn clear(&mut self) {
        self.sinks = false;
        self.event_sources = false;
        self.events.clear();
        self.program_ids.clear();
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::exec::program_event::ProgramEvent;
    use crate::exec::seeds::RunSeeds;
    use crate::graph::program_id::ProgramId;

    impl RunSeeds {
        pub(crate) fn sinks() -> Self {
            Self {
                sinks: true,
                ..Self::default()
            }
        }

        pub(crate) fn nodes(program_ids: Vec<ProgramId>) -> Self {
            Self {
                program_ids,
                ..Self::default()
            }
        }

        pub(crate) fn events(events: Vec<ProgramEvent>) -> Self {
            Self {
                events,
                ..Self::default()
            }
        }
    }
}
