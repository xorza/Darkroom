//! The user-facing outcome log shared by every frontend, owned by the
//! [`RuntimeHost`](crate::core::runtime_host::RuntimeHost): the last failure as
//! a sticky slot, which the GUI's status bar renders until a subsequent
//! success clears it.
//!
//! **`tracing` is the record.** Every entry is emitted through it, so the
//! structured log is the complete history regardless of frontend and nothing
//! here has to keep one. The history beside the slot is `cfg(test)` only: it exists so a test can assert *which* failures a path
//! reported, which the slot cannot express — it holds one at a time, and paths
//! like `OpenDocument::open_at_launch` report two.

#[derive(Debug, Default)]
pub(crate) struct StatusLog {
    /// The last failure, sticky until a subsequent success of the same
    /// family (a run kick, a finished run, a file op) assigns `None`.
    pub(crate) error: Option<String>,
    /// Every failure reported, oldest first. Test-only — see the module doc. A
    /// `cfg`'d field rather than a gated wrapper because it cannot move: it is
    /// the one piece of `StatusLog` that only tests observe.
    #[cfg(test)]
    lines: Vec<String>,
}

impl StatusLog {
    /// Record a failure: error-logged through `tracing`, and parked in the
    /// sticky [`error`](Self::error) slot.
    pub(crate) fn error(&mut self, line: String) {
        tracing::error!(target: "darkroom::status", "{line}");
        #[cfg(test)]
        self.lines.push(line.clone());
        self.error = Some(line);
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::core::status::StatusLog;

    impl StatusLog {
        /// The recorded history, oldest first. The status bar shows the sticky
        /// [`error`](StatusLog::error) slot alone, so this is read only by the
        /// tests that pin *which* failures a path reports.
        pub(crate) fn lines(&self) -> impl Iterator<Item = &str> {
            self.lines.iter().map(String::as_str)
        }
    }
}

#[cfg(test)]
mod tests;
