//! The document's share of a frame's requests.

use palantir::DockOp;

use crate::core::document::TabRef;
use crate::core::edit::document_request::DocumentRequest;
use crate::core::edit::graph_intent::GraphIntent;

/// The requests raised against the open document, in the order raised.
///
/// [`OpenDocument::drain_requests`](crate::core::document::open_document::OpenDocument::drain_requests)
/// takes them, three times a frame, so a request raised in one phase lands
/// before the next reads the document. The queue keeps its capacity, so a
/// frame's pushes allocate nothing once it has grown.
#[derive(Debug, Default)]
pub(crate) struct DocumentQueue {
    requests: Vec<DocumentRequest>,
}

impl DocumentQueue {
    pub(crate) fn push_graph(&mut self, intent: GraphIntent) {
        self.requests.push(DocumentRequest::Graph(intent));
    }

    pub(crate) fn extend_graph(&mut self, iter: impl IntoIterator<Item = GraphIntent>) {
        self.requests
            .extend(iter.into_iter().map(DocumentRequest::Graph));
    }

    pub(crate) fn push_view(&mut self, op: DockOp<TabRef>) {
        self.requests.push(DocumentRequest::View(op));
    }

    /// Take everything queued, in the order raised.
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = DocumentRequest> + '_ {
        self.requests.drain(..)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.requests.clear();
    }
}
