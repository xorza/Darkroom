//! Lifting one item to the front of the paint stack.

use scenarium::NodeId;
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// One item's paint depth, before and after.
///
/// Raising writes a depth past every other item rather than reordering a
/// list, so no neighbour moves and the two directions are the same write with
/// a different number — which a positional reorder could not promise.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Raise {
    pub(crate) key: NodeId,
    pub(crate) z: Change<u32>,
}

impl Reversible for Raise {
    fn write(&self, doc: &mut Document, dir: Direction) {
        if let Some(placement) = doc.main_view.item_placements.get_mut(&self.key) {
            placement.z = *self.z.half(dir);
        }
    }

    /// Clicking what is already frontmost lands here: the click still asks for
    /// the raise, and this is what keeps it out of the history.
    fn is_noop(&self) -> bool {
        self.z.unchanged()
    }

    /// Stacking rides in each item's depth and still writes on any save, like
    /// the selection does — but a bare restack shouldn't nag on exit.
    fn dirties_document(&self) -> bool {
        false
    }

    /// Reorders the paint stack; no node remeasures.
    fn invalidates_cached_geometry(&self) -> bool {
        false
    }
}
