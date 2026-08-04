//! The user-facing outcome log shared by every frontend, owned by the
//! [`RuntimeHost`](crate::core::runtime_host::RuntimeHost): the last failure as
//! a sticky slot, which the GUI's status bar renders until a subsequent
//! success clears it.
//!
//! **`tracing` is the record.** Every entry is emitted through it, so the
//! structured log is the complete history regardless of frontend and nothing
//! here has to keep one. The bounded rolling buffer beside the slot is
//! `cfg(test)` only: it exists so a test can assert *which* failures a path
//! reported, which the slot cannot express — it holds one at a time, and paths
//! like `OpenDocument::open_at_launch` report two.

#[derive(Debug, Default)]
pub(crate) struct StatusLog {
    /// The last failure, sticky until a subsequent success of the same
    /// family (a run kick, a finished run, a file op) assigns `None`.
    pub(crate) error: Option<String>,
    /// Rolling history, oldest first, capped by
    /// [`internals::STATUS_LOG_CAP`]. Test-only — see the module doc. A
    /// `cfg`'d field rather than a gated wrapper because it cannot move: it is
    /// the one piece of `StatusLog` that only tests observe.
    #[cfg(test)]
    lines: std::collections::VecDeque<String>,
}

impl StatusLog {
    /// Record a failure: error-logged through `tracing`, and parked in the
    /// sticky [`error`](Self::error) slot.
    pub(crate) fn error(&mut self, line: String) {
        tracing::error!(target: "darkroom::status", "{line}");
        #[cfg(test)]
        self.record(line.clone());
        self.error = Some(line);
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::core::status::StatusLog;

    /// Cap on the retained history (lines). Oldest lines drop off the front so
    /// a long-running test can't grow it without bound.
    pub(crate) const STATUS_LOG_CAP: usize = 200;

    impl StatusLog {
        /// The recorded history, oldest first. The status bar shows the sticky
        /// [`error`](StatusLog::error) slot alone, so this is read only by the
        /// tests that pin *which* failures a path reports.
        pub(crate) fn lines(&self) -> impl Iterator<Item = &str> {
            self.lines.iter().map(String::as_str)
        }

        /// Append to the capped history. Private to the gate: production never
        /// records one.
        pub(super) fn record(&mut self, line: String) {
            if self.lines.len() >= STATUS_LOG_CAP {
                self.lines.pop_front();
            }
            self.lines.push_back(line);
        }
    }
}

#[cfg(test)]
mod tests;
