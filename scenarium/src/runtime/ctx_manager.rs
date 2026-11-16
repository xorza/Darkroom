use common::CancelToken;

use crate::graph::program_id::ProgramId;
use crate::runtime::log_entry::{LogEntry, LogLevel};

#[derive(Debug, Default)]
pub struct CtxManager {
    /// Node currently being invoked, set by the executor before each
    /// lambda call so `log` can attribute lines. `None` outside a run.
    pub(crate) cur_program_id: Option<ProgramId>,
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
    pub const fn cur_program_id(&self) -> ProgramId {
        self.cur_program_id
            .expect("current_node is only readable inside a lambda invoke")
    }

    /// Emit a log line attributed to the node currently executing, mirrored to `tracing` so
    /// headless runs surface it. No-op outside a node invoke.
    pub fn log(&mut self, level: LogLevel, msg: impl Into<String>) {
        let Some(program_id) = self.cur_program_id else {
            return;
        };
        self.log_node(program_id, level, msg);
    }

    /// [`log`](Self::log) for a node the caller names, for what the executor reports about a node
    /// before or instead of running it, where `cur_program_id` is unset.
    pub(crate) fn log_node(
        &mut self,
        program_id: ProgramId,
        level: LogLevel,
        msg: impl Into<String>,
    ) {
        let message = msg.into();
        match level {
            LogLevel::Info => tracing::info!(?program_id, "{message}"),
            LogLevel::Warn => tracing::warn!(?program_id, "{message}"),
            LogLevel::Error => tracing::error!(?program_id, "{message}"),
        }
        self.logs.push(LogEntry {
            program_id,
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
    use crate::graph::program_id::ProgramId;
    use crate::runtime::ctx_manager::CtxManager;

    impl CtxManager {
        /// Stand in for the executor's per-invoke attribution, to test a lambda that reads
        /// [`CtxManager::cur_program_id`] without a run.
        pub const fn set_cur_program(&mut self, program_id: ProgramId) {
            self.cur_program_id = Some(program_id);
        }
    }
}
