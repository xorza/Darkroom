//! [`EventTrigger`]: an event source a successful run armed.

use crate::graph::func::event::EventLambda;
use crate::graph::identity::EventPort;
use crate::runtime::shared_any_state::SharedAnyState;

/// One `(event, lambda, state)` triple a successful event-source run armed, spawned by the worker
/// as a looping task. Carried out on
/// [`ExecutionOutcome::event_triggers`](crate::execution::report::execution_outcome::ExecutionOutcome::event_triggers),
/// which is why it lives in the report: the executor produces these without knowing the worker that
/// consumes them.
#[derive(Debug)]
pub(crate) struct EventTrigger {
    pub(crate) event: EventPort,
    pub(crate) lambda: EventLambda,
    pub(crate) state: SharedAnyState,
}
