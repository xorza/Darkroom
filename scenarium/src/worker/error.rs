//! What the worker reports when an operation fails, and what a caller gets when
//! there is no worker left to ask.
//!
//! [`WorkerError`] wraps an operation-level failure for the host — it always
//! arrives as a [`WorkerReport::Error`](crate::worker::protocol::WorkerReport),
//! never as a return value, because the worker runs the operation long after the
//! call that queued it. [`WorkerExited`] is the other direction: the send-side
//! failure every handle method returns once the task is gone.

use crate::execution::cache::runtime::error::{CacheFlushUnsupported, CacheNodeFailure};
use crate::execution::error::Error;

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("execution failed: {error}")]
    Execution {
        #[source]
        error: Error,
    },
    /// The nodes an eviction sweep could not clear, each still carrying its own
    /// cause. Never empty — a sweep with nothing to report sends no error.
    #[error("cache eviction failed for {} node(s): {}", failures.len(), join(failures))]
    CacheEviction { failures: Vec<CacheNodeFailure> },
    /// What a flush did not leave on disk, kept apart because the two mean
    /// different things to whoever asked: a failure may pass on the next write,
    /// while an unsupported type never will. Both empty sends no error.
    ///
    /// A host is free to present them the same way — `Display` does — but no
    /// longer has to, and the choice is now the host's rather than baked into a
    /// string the worker had already flattened.
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

/// The `details` rendering both cache variants share: one entry per node, in
/// the order the sweep met them.
fn join(entries: &[impl std::fmt::Display]) -> String {
    entries
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug, thiserror::Error)]
#[error("worker task has exited")]
pub struct WorkerExited;
