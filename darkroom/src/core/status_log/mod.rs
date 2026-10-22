//! The user-facing outcome log shared by every frontend, owned by the
//! frontend that renders it: the last failure or warning as a sticky slot,
//! which the GUI's status bar shows until a success of the same family clears
//! it.
//!
//! **`tracing` is the record.** Every entry is emitted through it, so the
//! structured log is the complete history regardless of frontend and nothing
//! here has to keep one. The history beside the slot is `cfg(test)` only: it
//! exists so a test can assert *which* failures a path reported, and how
//! many, which the slot cannot express — it holds one at a time.

/// What a failure, or the success that clears it, is about. A success clears
/// only its own family's failure, so a finished run cannot wipe the report of
/// a save that failed before it.
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
    /// Every failure reported, oldest first. Test-only — see the module doc. A
    /// `cfg`'d field rather than a gated wrapper because it cannot move: it is
    /// the one piece of `StatusLog` that only tests observe.
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
    /// Record a failure: error-logged through `tracing`, and parked in the
    /// sticky slot in place of whatever failure it held.
    pub(crate) fn error(&mut self, family: StatusFamily, line: String) {
        tracing::error!(target: "darkroom::status", "{line}");
        self.park(family, Severity::Error, line);
    }

    /// Record something the user has to know that is not a failure: logged
    /// as a warning, and parked in the sticky slot like a failure, so the next
    /// success of its family clears it.
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

#[cfg(test)]
pub(crate) mod internals {
    use crate::core::status_log::StatusLog;

    impl StatusLog {
        /// The recorded history, oldest first. The status bar shows the sticky
        /// slot alone, so this is read only by the tests that pin *which*
        /// failures a path reports.
        pub(crate) fn lines(&self) -> impl Iterator<Item = &str> {
            self.lines.iter().map(String::as_str)
        }
    }
}

#[cfg(test)]
mod tests;
