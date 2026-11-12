//! [`LogEntry`]: one node-attributed log record, and its [`LogLevel`].

use crate::graph::identity::NodeId;

/// How loud a log record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One node-attributed log record, written by
/// [`CtxManager::log`](crate::runtime::ctx_manager::CtxManager::log) while a lambda runs and
/// republished by [`RunSummary`](crate::worker::run_summary::RunSummary).
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub node_id: NodeId,
    pub level: LogLevel,
    pub message: String,
}
