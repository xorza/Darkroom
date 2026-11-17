//! [`SetCompoundName`]: renaming one compound.

use scenarium::{CompoundId, GraphId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::step::change::{Change, Direction};

/// A compound's name, before and after.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetCompoundName {
    pub(crate) id: CompoundId,
    pub(crate) name: Change<String>,
}

impl SetCompoundName {
    pub(super) fn write(&self, document: &mut Document, dir: Direction) {
        let replaced = document
            .graphs
            .rename_compound(self.id, self.name.half(dir).clone());
        debug_assert_eq!(
            &replaced,
            self.name.half(dir.reversed()),
            "history replays a rename over the name it recorded"
        );
    }

    pub(super) fn is_noop(&self) -> bool {
        self.name.unchanged()
    }

    /// A rename measures again: an instance with no name of its own is titled by the compound, in
    /// whichever graph places it. It writes the compound's graph alone, which an undo brings
    /// forward.
    pub(super) const fn facts(&self) -> EditFacts {
        EditFacts {
            renamed: true,
            touched: Touched::Graph(GraphId::Compound(self.id)),
            ..EditFacts::NONE
        }
    }
}
