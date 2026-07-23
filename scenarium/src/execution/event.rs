//! Runnable event state. An [`EventTrigger`] pairs an execution event port with
//! the lambda and shared state initialized by a successful event-source run.
//! It lives here so execution can produce triggers without depending on the
//! worker that consumes them.

use crate::execution::identity::ExecutionEventPort;
use crate::node::event::EventLambda;
use crate::runtime::shared_any_state::SharedAnyState;

/// One `(event, lambda, state)` triple spawned as a looping task.
#[derive(Debug)]
pub(crate) struct EventTrigger {
    pub(crate) event: ExecutionEventPort,
    pub(crate) lambda: EventLambda,
    pub(crate) state: SharedAnyState,
}
