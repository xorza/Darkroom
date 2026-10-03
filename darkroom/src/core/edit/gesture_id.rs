//! The identity of one held gesture.

/// One gesture a pointer holds open across frames: a drag from its press to
/// its release, or a run of wheel or pinch input.
///
/// The undo history folds a gesture's frames into one entry while their ids
/// match, so a held drag costs one Ctrl+Z, and two drags of the same node
/// stay two. [`DocumentQueue::open_gesture`] mints each id, and never mints
/// the same one twice. The default is the id before the first one minted, so
/// no gesture carries it.
///
/// [`DocumentQueue::open_gesture`]: crate::core::edit::document_queue::DocumentQueue::open_gesture
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GestureId(u64);

impl GestureId {
    /// The id after this one.
    pub(crate) const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}
