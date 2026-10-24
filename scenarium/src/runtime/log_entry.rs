//! [`LogEntry`]: one node-attributed log record, and its [`LogLevel`].

use crate::graph::identity::NodeId;

/// Where a log record came from and how loud it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One node-attributed log record.
/// [`CtxManager::log`](crate::runtime::ctx_manager::CtxManager::log) writes
/// these while a lambda runs; a run's
/// `ExecOutcome`
/// carries them out, and
/// [`RunSummary`](crate::worker::run_summary::RunSummary) republishes them.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub node_id: NodeId,
    pub level: LogLevel,
    pub message: String,
}
