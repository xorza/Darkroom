//! The user-facing outcome log shared by every frontend, owned by the
//! [`RuntimeHost`](crate::core::runtime_host::RuntimeHost): a bounded rolling history (the
//! TUI's `status` command renders it) plus the last failure as a sticky slot
//! (the GUI's status bar renders it, until a subsequent success clears it).
//! Every entry is also emitted through `tracing`, so the structured log stays
//! the complete record regardless of frontend.

use std::collections::VecDeque;

/// Cap on the retained history (lines). Oldest lines drop off the front so a
/// long-running session can't grow it without bound.
const STATUS_LOG_CAP: usize = 200;

#[derive(Debug, Default)]
pub(crate) struct StatusLog {
    /// Rolling history (worker summaries, script `print`s, failures).
    /// Private so every append goes through the cap in [`Self::push`];
    /// read via [`Self::lines`].
    lines: VecDeque<String>,
    /// The last failure, sticky until a subsequent success of the same
    /// family (a run kick, a finished run, a file op) assigns `None`.
    pub(crate) error: Option<String>,
}

impl StatusLog {
    /// Record a routine outcome: appended to the history and info-logged.
    pub(crate) fn info(&mut self, line: String) {
        tracing::info!(target: "darkroom::status", "{line}");
        self.push(line);
    }

    /// Record a failure: appended to the history, error-logged, and parked
    /// in the sticky [`error`](Self::error) slot.
    pub(crate) fn error(&mut self, line: String) {
        tracing::error!(target: "darkroom::status", "{line}");
        self.error = Some(line.clone());
        self.push(line);
    }

    pub(crate) fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    fn push(&mut self, line: String) {
        if self.lines.len() >= STATUS_LOG_CAP {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_slot_tracks_last_failure_and_history_keeps_both() {
        let mut log = StatusLog::default();

        // Routine lines never touch the sticky slot.
        log.info("run finished".into());
        assert_eq!(log.error, None);

        // A failure lands in both the slot and the history; a later failure
        // replaces the slot.
        log.error("save failed: a".into());
        log.error("compile failed: b".into());
        assert_eq!(log.error.as_deref(), Some("compile failed: b"));
        assert_eq!(
            log.lines().collect::<Vec<_>>(),
            ["run finished", "save failed: a", "compile failed: b"]
        );

        // Clearing the slot (a subsequent success) leaves the history intact.
        log.error = None;
        assert_eq!(log.error, None);
        assert_eq!(log.lines().count(), 3);
    }

    #[test]
    fn history_is_capped_dropping_oldest() {
        let mut log = StatusLog::default();
        // One over the cap: line "0" is evicted, "1"..=CAP remain.
        for i in 0..=STATUS_LOG_CAP {
            log.info(format!("{i}"));
        }
        assert_eq!(log.lines().count(), STATUS_LOG_CAP);
        assert_eq!(log.lines().next(), Some("1"));
        assert_eq!(
            log.lines().last(),
            Some(STATUS_LOG_CAP.to_string().as_str())
        );
    }
}
