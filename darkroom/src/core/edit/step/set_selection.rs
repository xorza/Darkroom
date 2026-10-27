//! Replacing the selection set.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::item_id::ItemId;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// The whole selection, before and after. The rubber band, node and pin
/// clicks, and Esc-deselect all land here: the caller computes the set it
/// wants and the step carries the one it replaced.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetSelection {
    pub(crate) selection: Change<BTreeSet<ItemId>>,
}

impl Reversible for SetSelection {
    fn write(&self, document: &mut Document, dir: Direction) {
        document
            .main_view
            .selected
            .clone_from(self.selection.half(dir));
    }

    fn is_noop(&self) -> bool {
        self.selection.unchanged()
    }

    /// Navigation: what is selected is view state the user does not "save".
    fn dirties_document(&self) -> bool {
        false
    }

    fn invalidates_cached_geometry(&self) -> bool {
        false
    }

    fn changes_node_set(&self) -> bool {
        false
    }
}
