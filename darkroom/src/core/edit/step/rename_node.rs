//! Renaming one node.

use scenarium::NodeId;
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// A node's display name, before and after.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RenameNode {
    pub(crate) node_id: NodeId,
    pub(crate) name: Change<String>,
}

impl Reversible for RenameNode {
    fn write(&self, doc: &mut Document, dir: Direction) {
        doc.graph.find_mut(self.node_id).unwrap().name = self.name.half(dir).clone();
    }

    fn is_noop(&self) -> bool {
        self.name.unchanged()
    }

    fn dirties_document(&self) -> bool {
        true
    }

    /// A title width change remeasures the header, shifting every port row
    /// below it.
    fn invalidates_cached_geometry(&self) -> bool {
        true
    }
}
