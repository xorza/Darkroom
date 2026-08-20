//! The contract every kind of undo step answers.

use crate::core::document::Document;
use crate::core::edit::step::change::Direction;
use crate::core::edit::step::gesture_key::GestureKey;
use crate::core::edit::step::undo_step::UndoStep;

/// One reversible edit primitive: a slot the document holds, plus what that
/// slot held before the edit and after it.
///
/// **This trait is the checklist for a new kind of edit.** Everything the
/// pipeline wants to know about a kind — how it writes, whether it changed
/// anything, what it costs the frame, whether it merges with the step before
/// it — is answered in that kind's own `impl`, so adding one is writing one
/// file rather than widening a match in five. The two questions with no safe
/// default have no default here either: a kind that forgets to answer them
/// does not compile.
///
/// Implementors are the payload structs behind
/// [`UndoStep`]'s variants; the enum forwards each call to the one it holds.
pub(super) trait Reversible {
    /// Write one half of this step into `doc`: the "to" half going forward,
    /// the "from" half going back.
    ///
    /// A single direction-parameterised write rather than an `apply`/`revert`
    /// pair, because the two are the same write with the other half of the
    /// [`Change`](crate::core::edit::step::change::Change) — spelling them
    /// separately is how they drift.
    ///
    /// Applying and then reverting must leave `doc` exactly as it was found:
    /// the action stack replays entries in reverse and the steps inside an
    /// entry in reverse, so each step is reverted against precisely the
    /// document its own apply produced. Steps that check themselves against
    /// what they overwrite rely on it.
    fn write(&self, doc: &mut Document, dir: Direction);

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

    /// Whether replaying this strands
    /// [`CanvasGeometry`](crate::gui::pane::graph::frame::geometry::CanvasGeometry)'s
    /// cross-frame caches: a widget whose *measured size* changed, or a node
    /// with no cached port offsets at all. Those caches are what wires anchor
    /// to, so a true answer costs one `ui.request_relayout()` at the end of
    /// `Editor::frame` and a second pass that rebuilds them against the first
    /// pass's arranged rects.
    ///
    /// A step that only *moves* or reorders answers false even though it does
    /// change what the layout engine reads: a port center resolves as
    /// `node.pos + cached offset`, and a move touches only `pos`. Getting that
    /// wrong is expensive rather than incorrect — a node drag emits one step
    /// per gesture frame, so a spurious true doubles the whole editor pipeline
    /// for the length of the drag.
    fn invalidates_cached_geometry(&self) -> bool;

    /// This step's continuous-gesture identity, or `None` when it is always
    /// its own undo entry. A kind that answers `Some` must also implement
    /// [`Self::coalesce`], since the action stack asks for the fold as soon as
    /// two keys match.
    fn gesture_key(&self) -> Option<GestureKey> {
        None
    }

    /// Fold `next` — the step that just arrived under the same
    /// [`Self::gesture_key`] — into this one: keep this step's "from" half and
    /// adopt `next`'s "to" half.
    ///
    /// The stack has already matched keys, so `next` is the same variant; the
    /// implementation re-checks anyway rather than trusting a caller with the
    /// invariant, and answers `None` if it isn't.
    fn coalesce(&self, next: &UndoStep) -> Option<UndoStep> {
        let _ = next;
        None
    }
}
