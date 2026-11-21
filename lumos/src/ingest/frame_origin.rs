//! [`FrameOrigin`]: where a set's frames come from.

/// Where a set's frames come from: the one word for it in every run's plan and log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameOrigin {
    /// Decoded from files into the run's tier, through the decode cache when the run keeps one.
    Files,
    /// Handed over decoded by the caller, resident from the start.
    Held,
    /// Stored by an earlier stage of the run, as a registered or drizzled set.
    Stored,
}
