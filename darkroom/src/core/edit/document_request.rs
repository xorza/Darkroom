//! One request a frontend raises against the open document.

use palantir::DockOperation;

use crate::core::document::tab_ref::TabRef;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;

/// A request the document applies itself: an edit of its graph, or of the
/// pane arrangement around it.
///
///   - [`Graph`](Self::Graph) — validated, applied, and recorded as an undo
///     step; flips the unsaved flag.
///   - [`InGesture`](Self::InGesture) — a graph edit that joins the undo
///     entry of a held gesture, as a preview spawn joins the drag that places
///     it.
///   - [`Cancel`](Self::Cancel) — Esc on a held gesture: its open entry is
///     reverted and dropped, so the gesture leaves nothing, else `otherwise`
///     applies, which puts back what it can.
///   - [`View`](Self::View) — applied in place, recording nothing and
///     dirtying nothing, so Ctrl+Z walks past a tab switch to the last graph
///     edit.
#[derive(Debug)]
pub(crate) enum DocumentRequest {
    Graph(GraphIntent),
    InGesture {
        gesture: GestureId,
        intent: GraphIntent,
    },
    Cancel {
        gesture: GestureId,
        otherwise: GraphIntent,
    },
    View(DockOperation<TabRef>),
}
