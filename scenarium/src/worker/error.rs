//! What the worker reports when an operation fails, and what a caller gets when
//! there is no worker left to ask.
//!
//! [`WorkerError`] always arrives as a
//! [`WorkerReport::Error`](crate::worker::protocol::WorkerReport), never as a return value,
//! because the worker runs the operation long after the call that queued it. [`WorkerExited`]
//! is the send-side failure every handle method returns once the task is gone, and
//! [`ExitError`] what [`Worker::exit`](crate::Worker::exit) answers.

use std::error;
use std::fmt::Display;

use common::ErrorChain;

use tokio::task::JoinError;

use crate::exec::cache::disk_store::error::PruneError;
use crate::exec::cache::runtime_cache::error::{CacheFlushUnsupported, CacheNodeFailure};
use crate::exec::error::ExecError;

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
    /// What a store's prune could not do, each with its own cause; never empty.
    #[error("cache prune failed {} time(s): {}", failures.len(), join_chains(failures))]
    CachePrune { failures: Vec<PruneError> },
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
}

/// The rendering both cache variants share: one entry per node, in the order the sweep met them.
fn join(entries: &[impl Display]) -> String {
    entries
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Each error with its causes, as one `source` cannot return them all.
fn join_chains(errors: &[impl error::Error]) -> String {
    errors
        .iter()
        .map(|error| ErrorChain::new(error).to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug, thiserror::Error)]
#[error("worker task has exited")]
pub struct WorkerExited;

/// Why [`Worker::exit`](crate::Worker::exit) did not see the task end cleanly.
#[derive(Debug, thiserror::Error)]
pub enum ExitError {
    /// The worker task panicked, a lambda's panic among them; the host resumes it.
    #[error("the worker task panicked")]
    Panicked(#[source] JoinError),
    /// A node was still running at the deadline, and the task was aborted.
    #[error("a node was still running when the worker was told to exit")]
    TimedOut,
}
