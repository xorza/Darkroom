//! One request a frontend raises against the open document.

use palantir::DockOperation;
use scenarium::{CompoundId, GraphId, InstancePath, ProgramId};

use crate::core::document::tab_ref::TabRef;
use crate::core::edit::compound_intent::CompoundIntent;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;

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
///   - [`Compound`](Self::Compound): an edit of the set of graphs, recorded in the same entry as
///     the graph edits around it.
///   - [`Enter`](Self::Enter): navigation as a view op is, into a compound's pane: it shows the
///     compound through `through`, opens or focuses its tab, and makes it the active graph.
///   - [`Show`](Self::Show): navigation, bringing a tab into view where it is or in its home;
///     see [`Document::show_tab`](crate::core::document::Document::show_tab).
///   - [`ShowPreview`](Self::ShowPreview): navigation, making a preview the active one and
///     showing the viewer that follows it.
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
    Compound(CompoundIntent),
    Enter {
        compound: CompoundId,
        through: Option<InstancePath>,
    },
    Show(TabRef),
    ShowPreview(ProgramId),
}
