//! Node-attributed log records produced during a run.
//!
//! A leaf of the execution tree: [`ContextManager::log`](crate::runtime::context::ContextManager::log)
//! writes these, [`ExecutionOutcome`](crate::execution::outcome::ExecutionOutcome) and
//! [`WorkerStatus`](crate::worker::status::WorkerStatus) carry them, and the host reads
//! them. Kept apart from the outcome that collects them so a lambda can log without
//! the runtime context depending on the whole completed-run surface.

use crate::execution::identity::ExecutionNodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub e_node_id: ExecutionNodeId,
    pub level: LogLevel,
    pub message: String,
}
