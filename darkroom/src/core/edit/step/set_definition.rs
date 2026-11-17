//! [`SetDefinition`]: a compound's definition and view replaced, under the same interface.

use scenarium::{Compound, CompoundId, GraphId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::graph_view::GraphView;
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::step::change::{Change, Direction};

/// A compound's definition and view, before and after, under the interface an interface edit
/// before it made the same: what an update from the library writes. Each instance keeps its
/// ports and bindings.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetDefinition {
    pub(crate) id: CompoundId,
    pub(crate) definition: Change<Compound>,
    pub(crate) view: Change<GraphView>,
}

impl SetDefinition {
    pub(super) fn write(&self, document: &mut Document, dir: Direction) {
        let (replaced, _) = document.replace_compound(
            self.id,
            self.definition.half(dir).clone(),
            self.view.half(dir).clone(),
        );
        debug_assert!(
            replaced == *self.definition.half(dir.reversed()),
            "a definition replaced is the one the step recorded"
        );
    }

    pub(super) fn is_noop(&self) -> bool {
        self.definition.unchanged() && self.view.unchanged()
    }

    /// The nodes inside come and go.
    pub(super) const fn facts(&self) -> EditFacts {
        EditFacts {
            node_set: true,
            touched: Touched::Graph(GraphId::Compound(self.id)),
            ..EditFacts::NONE
        }
    }
}
