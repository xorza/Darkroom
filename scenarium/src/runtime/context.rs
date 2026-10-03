use common::CancelToken;

use crate::execution::report::{LogEntry, LogLevel};
use crate::graph::identity::NodeId;

#[derive(Debug, Default)]
pub struct ContextManager {
    /// Node currently being invoked, set by the executor before each
    /// lambda call so `log` can attribute lines. `None` outside a run.
    pub(crate) current_node: Option<NodeId>,
    /// Log lines emitted this run, drained into `ExecutionOutcome` when the
    /// run finishes.
    pub(crate) logs: Vec<LogEntry>,
    /// The run's cooperative cancel token (the executor polls it between
    /// nodes). A lambda offloading heavy work can clone it via
    /// [`Self::cancel_flag`] and poll it inside that work to bail early.
    /// Defaults to a never-token outside a cancellable run.
    pub(crate) cancel: CancelToken,
}

impl ContextManager {
    /// A clonable handle to the run's [`CancelToken`], for a lambda to hand to
    /// long-running work (e.g. a `spawn_blocking` lumos op) so it can poll
    /// `token.is_cancelled()` and stop early. A never-token outside a
    /// cancellable run.
    pub fn cancel_flag(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// The node this lambda is running as — the same attribution [`Self::log`]
    /// stamps its lines with. For a lambda whose effect belongs to one authored
    /// node rather than to its return value: the only thing in
    /// [`Invocation`](crate::Invocation) that says *which* node is calling.
    ///
    /// Infallible because a lambda is the only place this is reachable, and the
    /// executor stamps the node immediately before every invoke — clearing it
    /// only once the whole run loop is done. Panicking here rather than handing
    /// back an `Option` keeps that invariant stated once, instead of every
    /// lambda re-deciding what an impossible `None` should mean.
    ///
    /// [`Self::log`] reads the field directly for the opposite reason: it is
    /// *also* callable from outside a run, and a dropped log line is harmless.
    ///
    /// # Panics
    /// Outside a node invoke — on a hand-built manager, or after a run.
    pub fn current_node(&self) -> NodeId {
        self.current_node
            .expect("current_node is only readable inside a lambda invoke")
    }

    /// Emit a log line attributed to the node currently executing, and
    /// mirror it to `tracing` at the matching level so headless runs
    /// still surface output. No-op when called outside a node invoke
    /// (`current_node` unset).
    pub fn log(&mut self, level: LogLevel, msg: impl Into<String>) {
        let Some(node_id) = self.current_node else {
            return;
        };
        self.log_node(node_id, level, msg);
    }

    /// [`log`](Self::log) for a node the caller names, rather than the one
    /// mid-invoke — what the executor reports *about* a node before or instead
    /// of running it, where `current_node` is not yet set and the no-op above
    /// would swallow the line.
    pub(crate) fn log_node(&mut self, node_id: NodeId, level: LogLevel, msg: impl Into<String>) {
        let message = msg.into();
        match level {
            LogLevel::Info => tracing::info!(?node_id, "{message}"),
            LogLevel::Warn => tracing::warn!(?node_id, "{message}"),
            LogLevel::Error => tracing::error!(?node_id, "{message}"),
        }
        self.logs.push(LogEntry {
            node_id,
            level,
            message,
        });
    }

    /// Sugar for [`Self::log`] at the matching level.
    pub fn info(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Info, msg);
    }
    pub fn warn(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Warn, msg);
    }
    pub fn error(&mut self, msg: impl Into<String>) {
        self.log(LogLevel::Error, msg);
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use crate::graph::identity::NodeId;
    use crate::runtime::context::ContextManager;

    impl ContextManager {
        /// Stand in for the executor's per-invoke attribution, so a lambda that
        /// reads [`ContextManager::current_node`] can be tested without a run.
        pub fn set_current_node(&mut self, node_id: NodeId) {
            self.current_node = Some(node_id);
        }
    }
}
