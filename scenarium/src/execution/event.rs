//! Event addressing and the runnable event triple. An [`EventRef`] names one
//! event port of one flat node in the [`ExecutionProgram`](crate::execution::program::ExecutionProgram);
//! an [`EventTrigger`] pairs that ref with the lambda and shared state needed to
//! run it. Both live here (not in `worker`) so `execution` stays self-contained
//! below the worker: the worker drives event triggers but does not define them.

use serde::{Deserialize, Serialize};

use crate::common::shared_any_state::SharedAnyState;
use crate::event_lambda::EventLambda;
use crate::graph::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventRef {
    pub node_id: NodeId,
    pub event_idx: usize,
}

/// One `(event, lambda, state)` triple spawned as a looping task.
#[derive(Debug)]
pub struct EventTrigger {
    pub event: EventRef,
    pub lambda: EventLambda,
    pub state: SharedAnyState,
}
