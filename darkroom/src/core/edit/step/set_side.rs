//! [`SetSide`]: a subgraph's place in the library, before and after.

use scenarium::{SubgraphDigest, SubgraphId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::{Change, Direction};

/// Whether a subgraph is shared, and the digest it last shared with the library, before and
/// after: what marking a subgraph shared, an update and a publish write beside its definition.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetSide {
    pub(crate) id: SubgraphId,
    pub(crate) shared: Change<bool>,
    pub(crate) synced: Change<Option<SubgraphDigest>>,
}

impl SetSide {
    pub(super) fn write(&self, document: &mut Document, dir: Direction) {
        let side = document
            .sides
            .get_mut(&self.id)
            .expect("history replays a side change against the subgraph it recorded");
        side.shared = *self.shared.half(dir);
        side.synced = *self.synced.half(dir);
    }

    pub(super) fn is_noop(&self) -> bool {
        self.shared.unchanged() && self.synced.unchanged()
    }
}
