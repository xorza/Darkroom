//! The document's share of a frame's requests.

use palantir::DockOperation;
use scenarium::{CompoundId, GraphId, InstancePath, ProgramId};

use crate::core::document::tab_ref::TabRef;
use crate::core::edit::compound_intent::CompoundIntent;
use crate::core::edit::document_request::DocumentRequest;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;

/// The requests raised against the open document, in the order raised.
///
/// [`OpenDocument::drain_requests`](crate::core::open_document::OpenDocument::drain_requests)
/// takes them, three times a frame, so a request raised in one phase lands before the next reads
/// the document. The queue keeps its capacity, so a frame's pushes allocate nothing once it has
/// grown.
#[derive(Debug, Default)]
pub(crate) struct DocumentQueue {
    requests: Vec<DocumentRequest>,
    /// The last id [`Self::open_gesture`] minted. The queue outlives every
    /// document the frontend opens, so no two gestures share an id.
    last_gesture: GestureId,
}

impl DocumentQueue {
    /// A fresh id for a gesture that starts now.
    pub(crate) const fn open_gesture(&mut self) -> GestureId {
        self.last_gesture = self.last_gesture.next();
        self.last_gesture
    }

    /// Queue an edit of `graph`.
    pub(crate) fn push_graph(&mut self, graph: GraphId, intent: GraphIntent) {
        self.requests.push(DocumentRequest::Graph { graph, intent });
    }

    /// Queue every edit of `graph` `iter` yields.
    pub(crate) fn extend_graph(
        &mut self,
        graph: GraphId,
        iter: impl IntoIterator<Item = GraphIntent>,
    ) {
        self.requests.extend(
            iter.into_iter()
                .map(|intent| DocumentRequest::Graph { graph, intent }),
        );
    }

    /// Queue every edit of `graph` `iter` yields as part of the held `gesture`, whose undo entry
    /// they join.
    pub(crate) fn extend_graph_in(
        &mut self,
        gesture: GestureId,
        graph: GraphId,
        iter: impl IntoIterator<Item = GraphIntent>,
    ) {
        self.requests
            .extend(iter.into_iter().map(|intent| DocumentRequest::InGesture {
                gesture,
                graph,
                intent,
            }));
    }

    /// Queue the cancel of the held `gesture` on `graph`: its open undo entry is reverted and
    /// dropped, else `otherwise` applies.
    pub(crate) fn push_cancel(
        &mut self,
        gesture: GestureId,
        graph: GraphId,
        otherwise: GraphIntent,
    ) {
        self.requests.push(DocumentRequest::Cancel {
            gesture,
            graph,
            otherwise,
        });
    }

    pub(crate) fn push_view(&mut self, op: DockOperation<TabRef>) {
        self.requests.push(DocumentRequest::View(op));
    }

    /// Queue an edit of the set of graphs.
    pub(crate) fn push_compound(&mut self, intent: CompoundIntent) {
        self.requests.push(DocumentRequest::Compound(intent));
    }

    /// Queue the entry into `compound`'s pane, shown through `through`.
    pub(crate) fn push_enter(&mut self, compound: CompoundId, through: Option<InstancePath>) {
        self.requests
            .push(DocumentRequest::Enter { compound, through });
    }

    /// Queue bringing `tab` into view.
    pub(crate) fn push_show(&mut self, tab: TabRef) {
        self.requests.push(DocumentRequest::Show(tab));
    }

    /// Queue making `program_id` the active preview, its viewer shown.
    pub(crate) fn push_show_preview(&mut self, program_id: ProgramId) {
        self.requests.push(DocumentRequest::ShowPreview(program_id));
    }

    /// Take everything queued, in the order raised.
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = DocumentRequest> + '_ {
        self.requests.drain(..)
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }
}
