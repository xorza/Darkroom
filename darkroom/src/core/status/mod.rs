//! The user-facing outcome log shared by every frontend, owned by the
//! frontend that renders it: the last failure as a sticky slot, which the
//! GUI's status bar shows until a success of the same family clears it.
//!
//! **`tracing` is the record.** Every entry is emitted through it, so the
//! structured log is the complete history regardless of frontend and nothing
//! here has to keep one. The history beside the slot is `cfg(test)` only: it
//! exists so a test can assert *which* failures a path reported, which the
//! slot cannot express — it holds one at a time, and paths like
//! `OpenDocument::open_at_launch` report two.

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
}

#[derive(Debug, Default)]
pub(crate) struct StatusLog {
    /// The last failure, sticky until a success of its family.
    error: Option<StatusError>,
    /// Every failure reported, oldest first. Test-only — see the module doc. A
    /// `cfg`'d field rather than a gated wrapper because it cannot move: it is
    /// the one piece of `StatusLog` that only tests observe.
    #[cfg(test)]
    lines: Vec<String>,
}

/// One failure in the sticky slot, and the family whose success clears it.
#[derive(Debug)]
struct StatusError {
    family: StatusFamily,
    line: String,
}

impl StatusLog {
    /// Record a failure: error-logged through `tracing`, and parked in the
    /// sticky slot in place of whatever failure it held.
    pub(crate) fn error(&mut self, family: StatusFamily, line: String) {
        tracing::error!(target: "darkroom::status", "{line}");
        #[cfg(test)]
        self.lines.push(line.clone());
        self.error = Some(StatusError { family, line });
    }

    /// Record a success: it clears the slot when the failure there is of the
    /// same family, and leaves any other family's failure standing.
    pub(crate) fn succeeded(&mut self, family: StatusFamily) {
        if self
            .error
            .as_ref()
            .is_some_and(|error| error.family == family)
        {
            self.error = None;
        }
    }

    /// The failure the status bar shows.
    pub(crate) fn current(&self) -> Option<&str> {
        self.error.as_ref().map(|error| error.line.as_str())
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::core::status::StatusLog;

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
