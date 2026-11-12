//! Toggling items in and out of the selection.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::item_id::ItemId;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::Direction;
use crate::core::edit::step::reversible::Reversible;

/// The items a selection change toggled: the symmetric difference of the sets before and after.
/// The caller computes the set it wants and the step keeps what changed, a Shift-click's one item
/// rather than both whole sets. Toggling is its own inverse, so one write serves both directions.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetSelection {
    pub(crate) toggled: BTreeSet<ItemId>,
}

impl Reversible for SetSelection {
    fn write(&self, document: &mut Document, _: Direction) {
        let selected = &mut document.main_view.selected;
        for item in &self.toggled {
            if !selected.remove(item) {
                selected.insert(*item);
            }
        }
    }

    fn is_noop(&self) -> bool {
        self.toggled.is_empty()
    }

    /// Navigation: what is selected is view state the user does not "save".
    fn dirties_document(&self) -> bool {
        false
    }

    fn facts(&self) -> EditFacts {
        EditFacts::NONE
    }
}
