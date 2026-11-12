use common::CancelToken;

use crate::graph::identity::NodeId;
use crate::runtime::log_entry::{LogEntry, LogLevel};

#[derive(Debug, Default)]
pub struct CtxManager {
    /// Node currently being invoked, set by the executor before each
    /// lambda call so `log` can attribute lines. `None` outside a run.
    pub(crate) cur_node_id: Option<NodeId>,
    /// Log lines emitted this run, drained into `ExecOutcome` when the
    /// run finishes.
    pub(crate) logs: Vec<LogEntry>,
    /// The run's cooperative cancel token, polled by the executor between nodes; a never-token
    /// outside a cancellable run. See [`Self::cancel_flag`].
    pub(crate) cancel: CancelToken,
}

impl CtxManager {
    /// A clonable handle to the run's [`CancelToken`], for a lambda to hand to long-running work
    /// that polls `token.is_cancelled()` to stop early. A never-token outside a cancellable run.
    pub fn cancel_flag(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// The node this lambda is running as, the attribution [`Self::log`] stamps its lines with:
    /// the only thing in [`Invocation`](crate::Invocation) that says *which* node is calling.
    ///
    /// Infallible because the executor stamps the node before every invoke, so an `Option` would
    /// only make every lambda re-decide what an impossible `None` means. [`Self::log`] reads the
    /// field directly because it is also callable outside a run, where a dropped line is harmless.
    ///
    /// # Panics
    /// Outside a node invoke — on a hand-built manager, or after a run.
    pub const fn cur_node_id(&self) -> NodeId {
        self.cur_node_id
            .expect("current_node is only readable inside a lambda invoke")
    }

    /// Emit a log line attributed to the node currently executing, mirrored to `tracing` so
    /// headless runs surface it. No-op outside a node invoke.
    pub fn log(&mut self, level: LogLevel, msg: impl Into<String>) {
        let Some(node_id) = self.cur_node_id else {
            return;
        };
        self.log_node(node_id, level, msg);
    }

    /// [`log`](Self::log) for a node the caller names, for what the executor reports about a node
    /// before or instead of running it, where `cur_node_id` is unset.
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

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::graph::identity::NodeId;
    use crate::runtime::ctx_manager::CtxManager;

    impl CtxManager {
        /// Stand in for the executor's per-invoke attribution, to test a lambda that reads
        /// [`CtxManager::cur_node_id`] without a run.
        pub const fn set_cur_node(&mut self, node_id: NodeId) {
            self.cur_node_id = Some(node_id);
        }
    }
}
