//! The contract every kind of undo step answers.

use crate::core::document::Document;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::Direction;

/// One reversible edit primitive: a slot the document holds, plus what that
/// slot held before the edit and after it.
///
/// **This trait is the checklist for a new kind of edit.** Everything the
/// pipeline wants to know about a kind — how it writes, whether it changed
/// anything, what it changed of the document's shape — is answered in that kind's own `impl`,
/// so adding one is writing one file rather than widening a match in five.
/// No question has a default: a kind that forgets to answer one does not
/// compile.
///
/// Implementors are the payload structs behind
/// [`UndoStep`](crate::core::edit::step::undo_step::UndoStep)'s variants; the
/// enum forwards each call to the one it holds.
pub(super) trait Reversible {
    /// Write one half of this step into `document`: the "to" half going forward,
    /// the "from" half going back.
    ///
    /// A single direction-parameterised write rather than an `apply`/`revert`
    /// pair, because the two are the same write with the other half of the
    /// [`Change`](crate::core::edit::step::change::Change) — spelling them
    /// separately is how they drift.
    ///
    /// Applying and then reverting must leave `document` exactly as it was found:
    /// the undo history replays entries in reverse and the steps inside an
    /// entry in reverse, so each step is reverted against precisely the
    /// document its own apply produced. Steps that check themselves against
    /// what they overwrite rely on it.
    fn write(&self, document: &mut Document, dir: Direction);

    /// Whether writing either half would leave the document unchanged.
    /// Filtered out at commit time so phantom entries — re-selecting the
    /// same node, dragging zero pixels — never reach the history and cost a
    /// Ctrl+Z that appears to do nothing.
    fn is_noop(&self) -> bool;

    /// Whether replaying this — in either direction — changes *saved*
    /// document content, as opposed to pure navigation (camera, selection,
    /// stacking) that isn't worth prompting to save on exit. Drives
    /// `OpenDocument::dirty`.
    fn dirties_document(&self) -> bool;

    /// What replaying this, in either direction, changes of the document's shape. The node
    /// set fact also sweeps the canvas's node caches, so a false one leaves a removed node's
    /// entries behind.
    fn facts(&self) -> EditFacts;
}
