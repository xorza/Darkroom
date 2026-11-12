//! The contract every kind of undo step answers.

use crate::core::document::Document;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::Direction;

/// One reversible edit primitive: a slot the document holds, plus what that
/// slot held before the edit and after it.
///
/// **This trait is the checklist for a new kind of edit.** Everything the pipeline wants to know
/// about a kind is answered in that kind's own `impl`, so adding one is writing one file rather
/// than widening a match in five. No question has a default: a kind that forgets one does not
/// compile.
///
/// Implementors are the payload structs behind
/// [`UndoStep`](crate::core::edit::step::undo_step::UndoStep)'s variants.
pub(super) trait Reversible {
    /// Write one half of this step into `document`: the "to" half going forward,
    /// the "from" half going back.
    ///
    /// One direction-parameterised write rather than an `apply`/`revert` pair: the two are the
    /// same write with the other half of the [`Change`](crate::core::edit::step::change::Change),
    /// and spelling them separately is how they drift.
    ///
    /// Applying then reverting must leave `document` exactly as found: the history replays
    /// entries and the steps inside an entry in reverse, so each step is reverted against the
    /// document its own apply produced. Steps that check themselves against what they overwrite
    /// rely on it.
    fn write(&self, document: &mut Document, dir: Direction);

    /// Whether writing either half would leave the document unchanged. Filtered out at commit
    /// time so phantom entries (re-selecting the same node, dragging zero pixels) never reach the
    /// history as a Ctrl+Z that appears to do nothing.
    fn is_noop(&self) -> bool;

    /// Whether replaying this, in either direction, changes *saved* document content, as opposed
    /// to navigation (camera, selection, stacking) not worth a save prompt on exit. Drives
    /// `OpenDocument::is_dirty`.
    fn dirties_document(&self) -> bool;

    /// What replaying this, in either direction, changes of the document's shape. The node set
    /// fact also sweeps the canvas's node caches, so a false one leaves a removed node's entries.
    fn facts(&self) -> EditFacts;
}
