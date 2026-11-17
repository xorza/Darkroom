//! [`SetSide`]: a compound's marks beside its definition, before and after.

use scenarium::CompoundId;
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::compound_side::SideMarks;
use crate::core::edit::step::change::{Change, Direction};

/// A compound's [`SideMarks`] before and after: what marking it shared, following the library,
/// locking it, an update and a publish write beside its definition.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetSide {
    pub(crate) id: CompoundId,
    pub(crate) marks: Change<SideMarks>,
}

impl SetSide {
    pub(super) fn write(&self, document: &mut Document, dir: Direction) {
        document
            .sides
            .get_mut(&self.id)
            .expect("history replays a side change against the compound it recorded")
            .set_marks(*self.marks.half(dir));
    }

    pub(super) fn is_noop(&self) -> bool {
        self.marks.unchanged()
    }
}
