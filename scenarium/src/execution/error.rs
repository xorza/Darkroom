use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::graph::identity::{EventPort, FuncId, NodeId};

/// An **operation-level** failure that aborts a whole plan / run: the schedule has a
/// cycle ([`CycleDetected`](Error::CycleDetected)), a node seed had no occurrence
/// ([`NodeSeedNotFound`](Error::NodeSeedNotFound)), an event seed had no port
/// ([`EventSeedNotFound`](Error::EventSeedNotFound)), or the event loop's lambda
/// panicked ([`EventLambdaPanic`](Error::EventLambdaPanic)). It's the error type of the
/// `Result`-returning entry points on both sides of the worker boundary — the engine's
/// plan/execute, and the worker operations around them, which is where the event-loop
/// panic is caught. A *single node's* run failure is a [`RunError`], carried by that
/// node's [`NodeStatus`](crate::execution::report::NodeStatus) row,
/// never one of these; a graph that won't compile is a
/// [`CompileError`](crate::execution::compile::error::CompileError), produced on the host before anything
/// reaches the engine — the phases can't be confused at the type level.
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum Error {
    #[error("Cycle detected while building execution graph at node {node_id:?}")]
    CycleDetected { node_id: NodeId },
    /// An execution-node seed is absent from the installed compiled program. A stale
    /// identity fails the run rather than being silently skipped.
    #[error("node seed {node_id:?} not found in the compiled program")]
    NodeSeedNotFound { node_id: NodeId },
    #[error("event seed {event:?} not found in the compiled program")]
    EventSeedNotFound { event: EventPort },
    #[error("event lambda for node {node_id:?} panicked: {message}")]
    EventLambdaPanic { node_id: NodeId, message: String },
}

/// A **single node's** run-time failure, reported in that node's one
/// [`NodeStatus`](crate::execution::report::NodeStatus) row as
/// [`NodeExecutionStatus::Errored`](crate::execution::report::NodeExecutionStatus::Errored).
/// Distinct from [`Error`](enum@Error) (whole-operation failures): a `RunError` always
/// concerns exactly one node, so it can't carry a compile/plan failure, and a caller
/// reading a node's row can't mistake a setup failure for a node's outcome.
#[derive(Debug, Error, Clone, Serialize, Deserialize)]
pub enum RunError {
    #[error("{message}")]
    Invoke { func_id: FuncId, message: String },
    // The messages omit `func_id` (kept as machine-readable data): a `RunError`
    // is already paired with its `NodeId` in the node's status row, so these
    // surface to the editor attributed to the node — a raw id in the text would be noise.
    /// The node's func was registered without an implementation
    /// ([`FuncLambda::None`](crate::graph::func::lambda::FuncLambda)), so the node
    /// can't execute. A host/library configuration error, reported per-node
    /// (its consumers skip as errored-upstream) rather than crashing the run.
    #[error("the node's function has no implementation attached")]
    MissingLambda { func_id: FuncId },
    #[error("skipped: an upstream dependency errored")]
    SkippedUpstream { func_id: FuncId },
    /// A disk blob the resolver verified by header no longer loaded when the run loop
    /// reached the node — deleted or corrupted in between. The reuse verdict already cut
    /// this node's producers, so the run can't fall back to recomputing it; the undecodable
    /// blob is dropped, so the next run misses cleanly.
    #[error("the node's cached output could not be loaded")]
    CacheLoadFailed { func_id: FuncId },
    /// A filesystem path this node declares could not be identified — the
    /// walk that keys its cache hit an I/O failure. Reported rather than
    /// left silently uncached, and attributed here rather than aborting
    /// the run: the node's dependents skip as errored-upstream, and every
    /// unrelated node still runs.
    #[error("a declared filesystem path could not be identified: {message}")]
    ResourceUnavailable { func_id: FuncId, message: String },
    #[error("demanded outputs {outputs:?} were left unbound")]
    OutputsNotProduced {
        func_id: FuncId,
        outputs: Vec<usize>,
    },
    #[error("cancelled before completing")]
    Cancelled { func_id: FuncId },
}

pub type Result<T> = std::result::Result<T, Error>;
