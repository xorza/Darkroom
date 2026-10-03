//! Dragging node bodies across the canvas.

use glam::Vec2;
use scenarium::NodeId;
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// One dragged member: where it sat, and where the drag put it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Move {
    pub(crate) key: NodeId,
    pub(crate) pos: Change<Vec2>,
}

/// A drag of one or more node bodies in canvas-world coordinates.
///
/// A multi-select drag moves the whole group as a single undo entry; a plain
/// drag carries just the one grabbed body.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MoveSelection {
    /// One entry per member that still had a placement when the step was
    /// built, in the drag's member order. A member whose node vanished
    /// mid-drag is dropped rather than refusing the whole move, so this can be
    /// shorter than the intent that produced it — and empty, which
    /// [`Reversible::is_noop`] then filters out.
    pub(crate) moves: Vec<Move>,
}

impl MoveSelection {
    /// Fold `next`, a later frame of the same drag, into this step: keep each
    /// member's `from` and adopt its latest `to`.
    ///
    /// Both frames list their members in the drag's order, and `next` lists a
    /// subset — a member can vanish mid-drag but never join — so one walk
    /// pairs them. A member `next` no longer carries keeps the position this
    /// step last gave it.
    pub(crate) fn absorb(&mut self, next: &Self) {
        let mut later = next.moves.iter().peekable();
        for moved in &mut self.moves {
            if let Some(update) = later.next_if(|update| update.key == moved.key) {
                moved.pos.to = update.pos.to;
            }
        }
        debug_assert!(
            later.next().is_none(),
            "a later frame of a drag moved a member the drag did not start with"
        );
    }
}

impl Reversible for MoveSelection {
    fn write(&self, doc: &mut Document, dir: Direction) {
        for moved in &self.moves {
            // A member removed since the step was recorded simply doesn't
            // move: undo restores it before the entry that placed it here.
            if let Some(placement) = doc.main_view.item_placements.get_mut(&moved.key) {
                placement.pos = *moved.pos.half(dir);
            }
        }
    }

    fn is_noop(&self) -> bool {
        self.moves.iter().all(|moved| moved.pos.unchanged())
    }

    fn dirties_document(&self) -> bool {
        true
    }

    /// Nothing remeasures: every member keeps its size and its cached
    /// intra-node offsets, and the canvas recomputes port centers from this
    /// frame's `pos`. The drag also drains pre-record, so the first pass
    /// already arranges at the cursor.
    fn invalidates_cached_geometry(&self) -> bool {
        false
    }

    fn retypes_outputs(&self) -> bool {
        false
    }
}
