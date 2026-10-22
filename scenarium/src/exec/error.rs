use std::sync::Arc;

use thiserror::Error;

use crate::graph::func::error::InvokeError;
use crate::graph::identity::NodeId;

/// An **operation-level** failure that aborts a whole plan / run: a node seed had no
/// occurrence ([`NodeSeedNotFound`](ExecError::NodeSeedNotFound)). It's the error type of the
/// engine's plan/execute, which the worker reports as
/// [`WorkerError::Exec`](crate::worker::error::WorkerError::Exec). A *single node's* run
/// failure is a [`RunError`], carried by that node's
/// [`NodeStatus`](crate::exec::report::node_status::NodeStatus) row, never one of these.
#[derive(Debug, Error, Clone)]
pub enum ExecError {
    /// An execution-node seed is absent from the installed compiled program. A stale
    /// identity fails the run rather than being silently skipped.
    #[error("node seed {node_id:?} not found in the compiled program")]
    NodeSeedNotFound { node_id: NodeId },
}

/// A **single node's** run-time failure, reported in that node's one
/// [`NodeStatus`](crate::exec::report::node_status::NodeStatus) row as
/// [`NodeExecStatus::Errored`](crate::exec::report::node_status::NodeExecStatus::Errored).
/// Distinct from [`ExecError`](enum@ExecError) (whole-operation failures): a `RunError` always concerns
/// exactly one node, so it can't carry a compile/plan failure, and a caller reading a node's row
/// can't mistake a setup failure for a node's outcome.
///
/// It names no node or func: it is always read beside the row's own `NodeId`.
#[derive(Debug, Error, Clone)]
pub enum RunError {
    /// The lambda failed, with the error it returned. Shared, so the row and
    /// every copy of the run summary carry the one error.
    #[error(transparent)]
    Invoke(Arc<InvokeError>),
    #[error("skipped: an upstream dependency errored")]
    SkippedUpstream,
    /// A disk blob the resolver verified by header no longer loaded when the run loop
    /// reached the node — deleted or corrupted in between. The reuse verdict already cut
    /// this node's producers, so the run can't fall back to recomputing it; the undecodable
    /// blob is dropped, so the next run misses cleanly.
    #[error("the node's cached output could not be loaded")]
    CacheLoadFailed,
    #[error("demanded outputs {outputs:?} were left unbound")]
    OutputsNotProduced { outputs: Vec<usize> },
    #[error("cancelled before completing")]
    Cancelled,
}

pub type ExecResult<T> = Result<T, ExecError>;
