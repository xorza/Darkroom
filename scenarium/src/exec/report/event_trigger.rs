//! [`EventTrigger`]: an event source a successful run armed.

use crate::exec::program_event::ProgramEvent;
use crate::graph::func::event::EventLambda;
use crate::runtime::shared_any_state::SharedAnyState;

/// One `(event, lambda, state)` triple a successful event-source run armed, spawned by the worker
/// as a looping task. Carried out on
/// [`ExecOutcome::event_triggers`](crate::exec::report::exec_outcome::ExecOutcome::event_triggers),
/// so the executor produces these without knowing the worker that consumes them.
#[derive(Debug)]
pub(crate) struct EventTrigger {
    pub(crate) event: ProgramEvent,
    pub(crate) lambda: EventLambda,
    pub(crate) state: SharedAnyState,
}
