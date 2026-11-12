//! The user-facing outcome log shared by every frontend, owned by the
//! frontend that renders it: the last failure or warning as a sticky slot,
//! which the GUI's status bar shows until a success of the same family clears
//! it.
//!
//! **`tracing` is the record.** Every entry is emitted through it, so the structured log is the
//! complete history and nothing here keeps one. A worker report's entry is emitted as the worker
//! posts it, and parked later through [`StatusLog::park_error`]. The history beside the slot is
//! `cfg(test)` only, so a test can assert *which* failures a path reported, and how many.

/// What a failure, or the success that clears it, is about. A success clears only its own
/// family's failure, so a finished run cannot wipe the report of an earlier failed save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusFamily {
    /// Compiling and running the graph.
    Run,
    /// Loading and saving the document.
    Document,
    /// Reading and writing the preferences file.
    Preferences,
    /// Setting a node input from a dialog's pick.
    Edit,
    /// Copying or saving an image of the window.
    View,
}

/// How the status bar paints a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    /// An action failed.
    Error,
    /// Something the user has to know that is not a failure, such as what a
    /// load repaired.
    Warning,
}

/// The line in the sticky slot, as the status bar reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StatusMessage<'a> {
    pub(crate) severity: Severity,
    pub(crate) line: &'a str,
}

#[derive(Debug, Default)]
pub(crate) struct StatusLog {
    /// The last failure or warning, sticky until a success of its family.
    slot: Option<StatusEntry>,
    /// Every failure reported, oldest first, which only tests read. A gated field, as a field
    /// cannot move to the end of the file.
    #[cfg(test)]
    lines: Vec<String>,
}

/// The line in the sticky slot, and the family whose success clears it.
#[derive(Debug)]
struct StatusEntry {
    family: StatusFamily,
    severity: Severity,
    line: String,
}

impl StatusLog {
    /// Record a failure: error-logged through `tracing`, and parked in the sticky slot in place
    /// of the failure it held.
    pub(crate) fn error(&mut self, family: StatusFamily, line: String) {
        log_error(&line);
        self.park_error(family, line);
    }

    /// Park a failure that [`log_error`] logged where it happened, in place of whatever failure
    /// the slot held: a worker report, logged as the worker posts it.
    pub(crate) fn park_error(&mut self, family: StatusFamily, line: String) {
        self.park(family, Severity::Error, line);
    }

    /// Record something the user has to know that is not a failure: logged as a warning, and
    /// parked in the sticky slot like a failure.
    pub(crate) fn warn(&mut self, family: StatusFamily, line: String) {
        tracing::warn!(target: "darkroom::status", "{line}");
        self.park(family, Severity::Warning, line);
    }

    fn park(&mut self, family: StatusFamily, severity: Severity, line: String) {
        #[cfg(test)]
        self.lines.push(line.clone());
        self.slot = Some(StatusEntry {
            family,
            severity,
            line,
        });
    }

    /// Record a success: it clears the slot when the line there is of the
    /// same family, and leaves any other family's line standing.
    pub(crate) fn succeeded(&mut self, family: StatusFamily) {
        if self
            .slot
            .as_ref()
            .is_some_and(|entry| entry.family == family)
        {
            self.slot = None;
        }
    }

    /// The line the status bar shows.
    pub(crate) fn cur(&self) -> Option<StatusMessage<'_>> {
        self.slot.as_ref().map(|entry| StatusMessage {
            severity: entry.severity,
            line: &entry.line,
        })
    }
}

/// Log `line` as [`StatusLog::error`] logs it, for a failure logged where it happens and parked
/// later through [`StatusLog::park_error`].
pub(crate) fn log_error(line: &str) {
    tracing::error!(target: "darkroom::status", "{line}");
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::core::status_log::StatusLog;

    impl StatusLog {
        /// The recorded history, oldest first.
        pub(crate) fn lines(&self) -> impl Iterator<Item = &str> {
            self.lines.iter().map(String::as_str)
        }
    }
}

#[cfg(test)]
mod tests;
