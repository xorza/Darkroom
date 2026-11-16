//! [`SubgraphSide`]: what the editor keeps beside one subgraph of the document.

use scenarium::{Graph as CoreGraph, InstancePath, SubgraphDigest};
use serde::{Deserialize, Serialize};

use crate::core::document::graph_view::GraphView;

/// What the editor keeps beside one subgraph: its view, as the main graph has one, the
/// instance its pane shows it through, and its place in the library. Kept in the document beside the scenarium subgraph, keyed
/// alike, as a view holds one placement per node: the view is the editor's, not the
/// framework's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SubgraphSide {
    pub(crate) view: GraphView,
    /// The instance whose run results the subgraph's pane shows, or `None` for none. View
    /// state, not undoable; cleared when it stops naming an instance of the subgraph.
    pub(crate) through: Option<InstancePath>,
    /// Whether the library lists this subgraph when the document is a file of the library
    /// folder.
    pub(crate) shared: bool,
    /// The digest this copy last shared with the library, by a place from it, an update or a
    /// publish; `None` for a subgraph made here. Read against the two current digests, it tells
    /// in sync, update available, changed here and changed in both apart.
    pub(crate) synced: Option<SubgraphDigest>,
}

impl SubgraphSide {
    /// A side for `graph` with its nodes placed in a row, shown through no instance.
    pub(crate) fn new(graph: &CoreGraph) -> Self {
        Self {
            view: GraphView::new(graph),
            through: None,
            shared: false,
            synced: None,
        }
    }
}
