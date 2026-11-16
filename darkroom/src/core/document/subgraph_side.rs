//! [`SubgraphSide`]: what the editor keeps beside one subgraph of the document.

use scenarium::Graph as CoreGraph;
use serde::{Deserialize, Serialize};

use crate::core::document::graph_view::GraphView;

/// What the editor keeps beside one subgraph: its view, as the main graph has one. Kept in the
/// document beside the scenarium subgraph, keyed alike, as a view holds one placement per node:
/// the view is the editor's, not the framework's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SubgraphSide {
    pub(crate) view: GraphView,
}

impl SubgraphSide {
    /// A side for `graph` with its nodes placed in a row.
    pub(crate) fn new(graph: &CoreGraph) -> Self {
        Self {
            view: GraphView::new(graph),
        }
    }
}
