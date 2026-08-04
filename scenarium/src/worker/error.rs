//! What the worker reports when an operation fails, and what a caller gets when
//! there is no worker left to ask.
//!
//! [`WorkerError`] wraps an operation-level failure for the host — it always
//! arrives as a [`WorkerReport::Error`](crate::worker::protocol::WorkerReport),
//! never as a return value, because the worker runs the operation long after the
//! call that queued it. [`WorkerExited`] is the other direction: the send-side
//! failure every handle method returns once the task is gone.

use crate::execution::error::Error;

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("execution failed: {error}")]
    Execution {
        #[source]
        error: Error,
    },
    #[error("cache eviction failed for {node_count} node(s): {details}")]
    CacheEviction { node_count: usize, details: String },
    #[error("cache flush wrote nothing for {node_count} node(s): {details}")]
    CacheFlush { node_count: usize, details: String },
}

#[derive(Debug, thiserror::Error)]
#[error("worker task has exited")]
pub struct WorkerExited;
