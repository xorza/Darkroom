//! The user-facing outcome log shared by every frontend, owned by the
//! [`RuntimeHost`](crate::core::runtime_host::RuntimeHost): a bounded rolling history (the
//! recorded for the log) plus the last failure as a sticky slot
//! (the GUI's status bar renders it, until a subsequent success clears it).
//! Every entry is also emitted through `tracing`, so the structured log stays
//! the complete record regardless of frontend.

use std::collections::VecDeque;

/// Cap on the retained history (lines). Oldest lines drop off the front so a
/// long-running session can't grow it without bound.
const STATUS_LOG_CAP: usize = 200;

#[derive(Debug, Default)]
pub(crate) struct StatusLog {
    /// Rolling history of failures, for the record.
    /// Private so every append goes through the cap in [`Self::push`];
    /// read via [`Self::lines`].
    lines: VecDeque<String>,
    /// The last failure, sticky until a subsequent success of the same
    /// family (a run kick, a finished run, a file op) assigns `None`.
    pub(crate) error: Option<String>,
}

impl StatusLog {
    /// Record a failure: appended to the history, error-logged, and parked
    /// in the sticky [`error`](Self::error) slot.
    pub(crate) fn error(&mut self, line: String) {
        tracing::error!(target: "darkroom::status", "{line}");
        self.error = Some(line.clone());
        self.push(line);
    }

    fn push(&mut self, line: String) {
        if self.lines.len() >= STATUS_LOG_CAP {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::core::status::StatusLog;

    impl StatusLog {
        /// The recorded history, oldest first. Test-only: the status bar
        /// shows the sticky [`error`](StatusLog::error) slot alone, so the
        /// history is written for the record and read only by the tests that
        /// pin *which* failures a path reports.
        pub(crate) fn lines(&self) -> impl Iterator<Item = &str> {
            self.lines.iter().map(String::as_str)
        }
    }
}

#[cfg(test)]
mod tests;
