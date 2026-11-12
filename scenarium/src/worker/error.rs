//! What the worker reports when an operation fails, and what a caller gets when
//! there is no worker left to ask.
//!
//! [`WorkerError`] always arrives as a
//! [`WorkerReport::Error`](crate::worker::protocol::WorkerReport), never as a return value,
//! because the worker runs the operation long after the call that queued it. [`WorkerExited`]
//! is the send-side failure every handle method returns once the task is gone.

use std::fmt::Display;

use crate::exec::cache::runtime_cache::error::{CacheFlushUnsupported, CacheNodeFailure};
use crate::exec::error::ExecError;
use crate::graph::identity::NodeId;

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("execution failed")]
    Exec {
        #[source]
        error: ExecError,
    },
    /// The nodes an eviction sweep could not clear, each carrying its own cause; never empty.
    #[error("cache eviction failed for {} node(s): {}", failures.len(), join(failures))]
    CacheEviction { failures: Vec<CacheNodeFailure> },
    /// What a flush did not leave on disk, kept apart because a failure may pass on the next
    /// write while an unsupported type never will; never both empty. `Display` presents them
    /// together.
    #[error(
        "cache flush wrote nothing for {} node(s): {}{}{}",
        failures.len() + unsupported.len(),
        join(failures),
        if failures.is_empty() || unsupported.is_empty() { "" } else { "; " },
        join(unsupported),
    )]
    CacheFlush {
        failures: Vec<CacheNodeFailure>,
        unsupported: Vec<CacheFlushUnsupported>,
    },
    /// An event lambda panicked, which stops the event loop. No run failed: the
    /// last completed run's results still stand.
    #[error("event lambda for node {node_id:?} panicked: {message}")]
    EventLambdaPanic { node_id: NodeId, message: String },
}

/// The rendering both cache variants share: one entry per node, in the order the sweep met them.
fn join(entries: &[impl Display]) -> String {
    entries
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug, thiserror::Error)]
#[error("worker task has exited")]
pub struct WorkerExited;
