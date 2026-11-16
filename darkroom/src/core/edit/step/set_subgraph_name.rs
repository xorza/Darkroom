//! [`SetSubgraphName`]: renaming one subgraph.

use scenarium::{GraphId, SubgraphId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::step::change::{Change, Direction};

/// A subgraph's name, before and after.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetSubgraphName {
    pub(crate) id: SubgraphId,
    pub(crate) name: Change<String>,
}

impl SetSubgraphName {
    pub(super) fn write(&self, document: &mut Document, dir: Direction) {
        let replaced = document
            .graphs
            .rename_subgraph(self.id, self.name.half(dir).clone());
        debug_assert_eq!(
            &replaced,
            self.name.half(dir.reversed()),
            "history replays a rename over the name it recorded"
        );
    }

    pub(super) fn is_noop(&self) -> bool {
        self.name.unchanged()
    }

    /// A rename measures again: an instance with no name of its own is titled by the subgraph, in
    /// whichever graph places it. It writes the subgraph's graph alone, which an undo brings
    /// forward.
    pub(super) const fn facts(&self) -> EditFacts {
        EditFacts {
            renamed: true,
            touched: Touched::Graph(GraphId::Subgraph(self.id)),
            ..EditFacts::NONE
        }
    }
}
