//! What a run schedule checks about itself.
//!
//! Every variant is a broken invariant of the two passes that fill the schedule
//! — a column that does not span the program, a node placed before the producer
//! it reads, a seed that is not a root. A caller cannot cause one, so these
//! surface only through the `is_debug()`-gated
//! [`validate_debug`](crate::execution::schedule::RunSchedule::validate_debug),
//! as values a test can assert on.

use thiserror::Error;

use crate::execution::identity::NodeIdx;
use crate::execution::schedule::NodeState;
use crate::graph::identity::NodeId;

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
    #[error("execution node {node_id:?} input range is out of bounds")]
    InputRange { node_id: NodeId },
    #[error("execution node {node_id:?} appears before dependency {dependency:?}")]
    BeforeDependency { node_id: NodeId, dependency: NodeId },
    #[error("execution node {node_id:?} appears more than once")]
    DuplicateNode { node_id: NodeId },
    #[error("unscheduled node {node_id:?} was decided {state:?}")]
    UnscheduledNodeDecided { node_id: NodeId, state: NodeState },
}
