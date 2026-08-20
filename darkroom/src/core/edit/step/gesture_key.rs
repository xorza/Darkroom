//! The identity that decides whether two consecutive steps are one gesture.

use scenarium::NodeId;

/// "Same continuous gesture", for undo coalescing: the action stack folds
/// consecutive single-step entries that report the same key into one entry,
/// so a drag held across thirty frames costs one Ctrl+Z rather than thirty.
///
/// Only the kinds a pointer can hold open have one — see
/// [`Reversible::gesture_key`](crate::core::edit::step::reversible::Reversible::gesture_key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GestureKey {
    Viewport,
    /// A group drag, keyed by whichever node the pointer latched, so two
    /// different grabbed nodes never coalesce.
    SelectionDrag(NodeId),
}
