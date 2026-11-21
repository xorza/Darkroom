use thiserror::Error;

use crate::decl::func::error::InvokeError;
use crate::identity::program_id::ProgramId;

/// An **operation-level** failure that aborts a whole plan / run: a node seed had no
/// occurrence ([`NodeSeedNotFound`](ExecError::NodeSeedNotFound)). It's the error type of the
/// engine's plan/execute, which the worker reports as
/// [`WorkerError::Exec`](crate::worker::error::WorkerError::Exec). A *single node's* run
/// failure is a [`RunError`], carried by that node's
/// [`NodeStatus`](crate::exec::report::node_status::NodeStatus) row, never one of these.
#[derive(Debug, Error)]
pub enum ExecError {
    /// An execution-node seed is absent from the installed compiled program. A stale
    /// identity fails the run rather than being silently skipped.
    #[error("node seed {program_id:?} not found in the compiled program")]
    NodeSeedNotFound { program_id: ProgramId },
}

/// A **single node's** run-time failure, reported in that node's one
/// [`NodeStatus`](crate::exec::report::node_status::NodeStatus) row as
/// [`NodeExecStatus::Errored`](crate::exec::report::node_status::NodeExecStatus::Errored).
/// Distinct from [`ExecError`](enum@ExecError) (whole-operation failures): a `RunError` always
/// concerns exactly one node, so a caller reading a node's row can't mistake a setup failure for
/// a node's outcome.
///
/// It names no node or func: it is always read beside the row's own `ProgramId`.
#[derive(Debug, Error)]
pub enum RunError {
    /// The lambda failed, with the error it returned.
    #[error(transparent)]
    Invoke(InvokeError),
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
    /// A lambda wrote a value its output's declared type does not hold: the producer's fault,
    /// reported against it, where a consumer reading the value as declared would panic.
    #[error("outputs {outputs:?} hold values their declared types do not")]
    OutputType { outputs: Vec<usize> },
    /// A path input holds a [`PathForm::Foreign`](crate::data::path_form::PathForm::Foreign) path,
    /// as a document saved on another system can. Refused before the lambda runs: this system
    /// would read it as something else, as Windows reads `/data/a.fit` on the current drive.
    #[error("input {idx} names {path:?}, a path this system cannot read as written")]
    ForeignPath { idx: usize, path: String },
    #[error("cancelled before completing")]
    Cancelled,
}

pub type ExecResult<T> = Result<T, ExecError>;
