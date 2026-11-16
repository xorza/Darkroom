//! One request a frontend raises against the open document.

use palantir::DockOperation;
use scenarium::{GraphId, InstancePath, SubgraphId};

use crate::core::document::tab_ref::TabRef;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::subgraph_intent::SubgraphIntent;

/// A request the document applies itself: an edit of one of its graphs, or of the pane
/// arrangement. A request against a graph makes it the active one.
///
///   - [`Graph`](Self::Graph): validated, applied, and recorded as an undo step; flips the
///     unsaved flag.
///   - [`InGesture`](Self::InGesture): a graph edit that joins the undo entry of a held gesture,
///     as a preview spawn joins the drag that places it.
///   - [`Cancel`](Self::Cancel): Esc on a held gesture; its open entry is reverted and dropped,
///     else `otherwise` applies.
///   - [`View`](Self::View): applied in place, recording and dirtying nothing, so Ctrl+Z walks
///     past a tab switch to the last graph edit.
///   - [`Subgraph`](Self::Subgraph): an edit of the set of graphs, recorded in the same entry as
///     the graph edits around it.
///   - [`Enter`](Self::Enter): navigation as a view op is, into a subgraph's pane: it shows the
///     subgraph through `through`, opens or focuses its tab, and makes it the active graph.
#[derive(Debug)]
pub(crate) enum DocumentRequest {
    Graph {
        graph: GraphId,
        intent: GraphIntent,
    },
    InGesture {
        gesture: GestureId,
        graph: GraphId,
        intent: GraphIntent,
    },
    Cancel {
        gesture: GestureId,
        graph: GraphId,
        otherwise: GraphIntent,
    },
    View(DockOperation<TabRef>),
    Subgraph(SubgraphIntent),
    Enter {
        subgraph: SubgraphId,
        through: Option<InstancePath>,
    },
}
