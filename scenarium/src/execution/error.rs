use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::execution::identity::{ExecutionEventPort, ExecutionNodeId};
use crate::node::definition::FuncId;

/// An **operation-level** failure that aborts a whole plan / run: the schedule has a
/// cycle ([`CycleDetected`](Error::CycleDetected)), a node seed had no occurrence
/// ([`NodeSeedNotFound`](Error::NodeSeedNotFound)), an event seed had no port
/// ([`EventSeedNotFound`](Error::EventSeedNotFound)), or the event loop's lambda
/// panicked ([`EventLambdaPanic`](Error::EventLambdaPanic)). It's the error type of the
/// engine's `Result`-returning entry points. A *single node's* run failure is a [`RunError`]
/// (collected into [`ExecutionOutcome::node_errors`](crate::execution::outcome::ExecutionOutcome)),
/// never one of these; a graph that won't compile is a
/// [`CompileError`](crate::execution::compile::CompileError), produced on the host before anything
/// reaches the engine — the phases can't be confused at the type level.
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum Error {
    #[error("Cycle detected while building execution graph at node {e_node_id:?}")]
    CycleDetected { e_node_id: ExecutionNodeId },
    /// An execution-node seed is absent from the installed compiled program. A stale
    /// identity fails the run rather than being silently skipped.
    #[error("node seed {e_node_id:?} not found in the compiled program")]
    NodeSeedNotFound { e_node_id: ExecutionNodeId },
    #[error("event seed {event:?} not found in the compiled program")]
    EventSeedNotFound { event: ExecutionEventPort },
    #[error("event lambda for node {e_node_id:?} panicked: {message}")]
    EventLambdaPanic {
        e_node_id: ExecutionNodeId,
        message: String,
    },
}

/// A **single node's** run-time failure, collected per-node into
/// [`ExecutionOutcome::node_errors`](crate::execution::outcome::ExecutionOutcome). Distinct
/// from [`Error`] (whole-operation failures): a `RunError` always concerns exactly one
/// node, so it can't carry a compile/plan failure, and a caller reading `node_errors`
/// can't mistake a setup failure for a node's outcome.
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum RunError {
    #[error("{message}")]
    Invoke { func_id: FuncId, message: String },
    // The messages omit `func_id` (kept as machine-readable data): a `RunError`
    // is already paired with its `ExecutionNodeId` in `node_errors`, so these surface to
    // the editor attributed to the node — a raw id in the text would be noise.
    /// The node's func was registered without an implementation
    /// ([`FuncLambda::None`](crate::node::lambda::FuncLambda)), so the node
    /// can't execute. A host/library configuration error, reported per-node
    /// (its consumers skip as errored-upstream) rather than crashing the run.
    #[error("the node's function has no implementation attached")]
    MissingLambda { func_id: FuncId },
    #[error("skipped: an upstream dependency errored")]
    SkippedUpstream { func_id: FuncId },
    #[error("demanded outputs {outputs:?} were left unbound")]
    OutputsNotProduced {
        func_id: FuncId,
        outputs: Vec<usize>,
    },
    #[error("cancelled before completing")]
    Cancelled { func_id: FuncId },
}

pub type Result<T> = std::result::Result<T, Error>;
