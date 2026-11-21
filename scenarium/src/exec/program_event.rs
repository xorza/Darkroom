//! [`ProgramEvent`]: an event of a program node.

use crate::identity::program_id::ProgramId;

/// An event of a program node: what the event loop fires and a run seeds, as an
/// [`EventPort`](crate::identity::EventPort) names one of an authored node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProgramEvent {
    pub(crate) program_id: ProgramId,
    pub(crate) event_idx: usize,
}

impl ProgramEvent {
    pub(crate) const fn new(program_id: ProgramId, event_idx: usize) -> Self {
        Self {
            program_id,
            event_idx,
        }
    }
}
