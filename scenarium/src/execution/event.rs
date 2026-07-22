//! Runnable event state. An [`EventTrigger`] pairs an execution event port with
//! the lambda and shared state needed to run it. It lives here (not in `worker`)
//! so `execution` stays self-contained below the worker: the worker drives event
//! triggers but does not define them.

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
