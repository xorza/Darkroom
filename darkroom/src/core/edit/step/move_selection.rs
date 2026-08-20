//! Dragging node bodies across the canvas.

use glam::Vec2;
use scenarium::NodeId;
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::gesture_key::GestureKey;
use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::undo_step::UndoStep;

/// One dragged member: where it sat, and where the drag put it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Move {
    pub(crate) key: NodeId,
    pub(crate) pos: Change<Vec2>,
}

/// A drag of one or more node bodies in canvas-world coordinates.
///
/// A multi-select drag moves the whole group as a single undo entry; a plain
/// drag carries just the one grabbed body. `grabbed` is whichever member the
/// pointer latched — it keys the gesture, so consecutive frames of one drag
/// coalesce while two different grabbed nodes stay separate entries.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MoveSelection {
    pub(crate) grabbed: NodeId,
    /// One entry per member that still had a placement when the step was
    /// built. A member whose node vanished mid-drag is dropped rather than
    /// refusing the whole move, so this can be shorter than the intent that
    /// produced it — and empty, which [`Self::is_noop`] then filters out.
    pub(crate) moves: Vec<Move>,
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

    fn gesture_key(&self) -> Option<GestureKey> {
        Some(GestureKey::SelectionDrag(self.grabbed))
    }

    fn coalesce(&self, next: &UndoStep) -> Option<UndoStep> {
        let UndoStep::MoveSelection(next) = next else {
            return None;
        };
        // A matched `SelectionDrag` key means the same grabbed member and so
        // the same group: keep each member's original `from` and adopt its
        // latest `to`. A member `next` no longer carries — it vanished
        // mid-drag — keeps the position this step last gave it.
        let moves = self
            .moves
            .iter()
            .map(|moved| Move {
                key: moved.key,
                pos: Change {
                    from: moved.pos.from,
                    to: next
                        .moves
                        .iter()
                        .find(|later| later.key == moved.key)
                        .map_or(moved.pos.to, |later| later.pos.to),
                },
            })
            .collect();
        Some(UndoStep::MoveSelection(Self {
            grabbed: self.grabbed,
            moves,
        }))
    }
}
