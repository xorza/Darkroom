//! [`SetSubgraph`]: a subgraph's definition and its side coming into the document or leaving it.

use scenarium::{GraphId, Subgraph, SubgraphId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::subgraph_side::SubgraphSide;
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::step::change::{Direction, EntryEdit};

/// A subgraph's whole entry, its definition and the side the editor keeps beside it, coming
/// into the document or leaving it. A removal holds only while no node places the subgraph:
/// the edit that removes the last instance comes first.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetSubgraph {
    id: SubgraphId,
    subgraph: Subgraph,
    side: SubgraphSide,
    edit: EntryEdit,
}

impl SetSubgraph {
    /// Put `subgraph` into the document under `id`, with `side`.
    pub(crate) const fn insertion(id: SubgraphId, subgraph: Subgraph, side: SubgraphSide) -> Self {
        Self {
            id,
            subgraph,
            side,
            edit: EntryEdit::Insertion,
        }
    }

    /// Take subgraph `id` out of `document`, which no node places.
    ///
    /// # Panics
    /// If the document holds no subgraph `id`.
    pub(crate) fn removal(document: &Document, id: SubgraphId) -> Self {
        Self {
            id,
            subgraph: document
                .graphs
                .subgraph(id)
                .expect("a removed subgraph is in the set")
                .clone(),
            side: document.sides[&id].clone(),
            edit: EntryEdit::Removal,
        }
    }

    pub(super) fn write(&self, document: &mut Document, dir: Direction) {
        if self.edit.puts_in(dir) {
            document.insert_subgraph(self.id, self.subgraph.clone(), self.side.clone());
        } else {
            let (subgraph, side) = document.remove_subgraph(self.id);
            debug_assert_eq!(
                (&subgraph, side.view == self.side.view),
                (&self.subgraph, true),
                "removal diverged from the step that recorded it"
            );
        }
    }

    /// What either half changes: a graph comes or goes, with every node of it.
    pub(super) const fn facts(&self) -> EditFacts {
        EditFacts {
            node_set: true,
            touched: Touched::Graph(GraphId::Subgraph(self.id)),
            ..EditFacts::NONE
        }
    }
}
