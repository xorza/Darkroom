//! One request a frontend raises against the open document.

use palantir::DockOp;

use crate::core::document::TabRef;
use crate::core::edit::graph_intent::GraphIntent;

/// A request the document applies itself: an edit of its graph, or of the
/// pane arrangement around it.
///
///   - [`Graph`](Self::Graph) — validated, applied, and recorded as an undo
///     step; flips the unsaved flag.
///   - [`View`](Self::View) — applied in place, recording nothing and
///     dirtying nothing, so Ctrl+Z walks past a tab switch to the last graph
///     edit.
#[derive(Debug)]
pub(crate) enum DocumentRequest {
    Graph(GraphIntent),
    View(DockOp<TabRef>),
}
