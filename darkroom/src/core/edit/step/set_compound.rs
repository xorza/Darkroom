//! [`SetCompound`]: a compound's definition and its side coming into the document or leaving it.

use scenarium::{Compound, CompoundId, GraphId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::compound_side::CompoundSide;
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::step::change::{Direction, EntryEdit};

/// A compound's whole entry, its definition and the side the editor keeps beside it, coming
/// into the document or leaving it. A removal holds only while no node places the compound:
/// the edit that removes the last instance comes first.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetCompound {
    id: CompoundId,
    compound: Compound,
    side: CompoundSide,
    edit: EntryEdit,
}

impl SetCompound {
    /// Put `compound` into the document under `id`, with `side`.
    pub(crate) const fn insertion(id: CompoundId, compound: Compound, side: CompoundSide) -> Self {
        Self {
            id,
            compound,
            side,
            edit: EntryEdit::Insertion,
        }
    }

    /// Take compound `id` out of `document`, which no node places.
    ///
    /// # Panics
    /// If the document holds no compound `id`.
    pub(crate) fn removal(document: &Document, id: CompoundId) -> Self {
        Self {
            id,
            compound: document
                .graphs
                .compound(id)
                .expect("a removed compound is in the set")
                .clone(),
            side: document.sides[&id].clone(),
            edit: EntryEdit::Removal,
        }
    }

    pub(super) fn write(&self, document: &mut Document, dir: Direction) {
        if self.edit.puts_in(dir) {
            document.insert_compound(self.id, self.compound.clone(), self.side.clone());
        } else {
            let (compound, side) = document.remove_compound(self.id);
            debug_assert_eq!(
                (&compound, side.view == self.side.view),
                (&self.compound, true),
                "removal diverged from the step that recorded it"
            );
        }
    }

    /// What either half changes: a graph comes or goes, with every node of it.
    pub(super) const fn facts(&self) -> EditFacts {
        EditFacts {
            node_set: true,
            touched: Touched::Graph(GraphId::Compound(self.id)),
            program: true,
            ..EditFacts::NONE
        }
    }
}
