//! Dragging node bodies and ties across the canvas.

use glam::Vec2;
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::item_id::ItemId;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// One dragged member: where it sat, and where the drag put it.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Move {
    pub(crate) item: ItemId,
    pub(crate) pos: Change<Vec2>,
}

/// A drag of one or more node bodies and ties in canvas-world coordinates.
///
/// A multi-select drag moves the whole group as a single undo entry; a plain
/// drag carries just the one grabbed body.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetPositions {
    /// One entry per member that still had a placement when the step was
    /// built, in the drag's member order. A member whose item vanished
    /// mid-drag is dropped rather than refusing the whole move, so this can be
    /// shorter than the intent that produced it — and empty, which
    /// [`Reversible::is_noop`] then filters out.
    pub(crate) moves: Vec<Move>,
}

impl SetPositions {
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
            if let Some(update) = later.next_if(|update| update.item == moved.item) {
                moved.pos.to = update.pos.to;
            }
        }
        #[expect(
            clippy::debug_assert_with_mut_call,
            reason = "nothing reads `later` after the check"
        )]
        {
            debug_assert!(
                later.next().is_none(),
                "a later frame of a drag moved a member the drag did not start with"
            );
        }
    }
}

impl Reversible for SetPositions {
    fn write(&self, document: &mut Document, dir: Direction) {
        let view = &mut document.main_view;
        for moved in &self.moves {
            let pos = match moved.item {
                ItemId::Node(node_id) => view.node_placements.get_mut(&node_id).map(|p| &mut p.pos),
                ItemId::Tie(tie) => view.ties.get_mut(&tie).map(|tie| &mut tie.pos),
            };
            *pos.expect("history replays a move against the items it recorded") =
                *moved.pos.half(dir);
        }
    }

    fn is_noop(&self) -> bool {
        self.moves.iter().all(|moved| moved.pos.unchanged())
    }

    fn dirties_document(&self) -> bool {
        true
    }

    fn facts(&self) -> EditFacts {
        EditFacts::NONE
    }
}
