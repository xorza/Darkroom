//! What a run schedule checks about itself.
//!
//! Every variant is a broken invariant of the two passes that fill the schedule
//! — a column that does not span the program, a node placed before the producer
//! it reads, a seed that is not a root. A caller cannot cause one, so these
//! surface only through the `is_debug()`-gated
//! [`validate_debug`](crate::execution::schedule::RunSchedule::validate_debug),
//! as values a test can assert on.

use thiserror::Error;

use crate::execution::identity::ExecutionNodeId;
use crate::execution::identity::NodeIdx;
use crate::execution::schedule::NodeState;

#[derive(Debug, Error)]
pub(crate) enum RunScheduleValidationError {
    #[error("execution order contains more entries than the program")]
    OrderTooLong,
    #[error("schedule {set} spans {len} entries, not the program's {expected}")]
    SetLength {
        set: &'static str,
        len: usize,
        expected: usize,
    },
    #[error("execution order contains an out-of-range node index: {node_idx:?}")]
    NodeOutOfRange { node_idx: NodeIdx },
    #[error("execution node {e_node_id:?} input range is out of bounds")]
    InputRange { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} appears before dependency {dependency:?}")]
    BeforeDependency {
        e_node_id: ExecutionNodeId,
        dependency: ExecutionNodeId,
    },
    #[error("execution node {e_node_id:?} appears more than once")]
    DuplicateNode { e_node_id: ExecutionNodeId },
    #[error("unscheduled node {e_node_id:?} was decided {state:?}")]
    UnscheduledNodeDecided {
        e_node_id: ExecutionNodeId,
        state: NodeState,
    },
    #[error("seeded node {e_node_id:?} is not an execution root")]
    SeededNodeNotRoot { e_node_id: ExecutionNodeId },
    #[error("event source {e_node_id:?} is not an execution root")]
    EventSourceNotRoot { e_node_id: ExecutionNodeId },
}
