//! The named results [`OpenDocument`](super::OpenDocument)'s methods hand back.

use crate::gui::relayout::Relayout;

/// What one undo/redo replay did.
///
/// Two independent facts, so a caller that only cares about one is not made to
/// guess from the other: a replay that found no entry still reports
/// `Relayout::NotNeeded` truthfully, and one that found an entry may or may
/// not have stranded the canvas's caches.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReplayOutcome {
    /// Whether there was an entry to replay at all. `false` means the history
    /// was already at its end in that direction.
    ///
    /// Read only by tests today — a chord that finds nothing has nothing to
    /// do, so production drops it. Kept because it is the only way to assert
    /// what *didn't* record an entry: that a dock op leaves the history
    /// untouched is a claim about the absence of a step, and no document
    /// state shows it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) took: bool,
    /// Whether the replayed steps stranded `CanvasGeometry`'s cross-frame
    /// caches.
    pub(crate) relayout: Relayout,
}
