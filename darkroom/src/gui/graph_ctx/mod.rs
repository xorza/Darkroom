//! The graph as the UI reads it: the document, resolved against the library
//! and the last run.
//!
//! Nothing here is copied or cached. A [`GraphCtx`] is the frame's
//! [`AppCtx`] plus the open document; the handles it hands out
//! ([`NodeCtx`], [`InputCtx`](input_ctx::InputCtx),
//! [`OutputCtx`](output_ctx::OutputCtx)) each resolve one more borrow and
//! answer every question
//! from the authority that owns it — the node's record off the document, its
//! ports off the library's declaration, its status off the run. A widget
//! therefore cannot read anything a frame behind, and there is nothing to
//! invalidate when the document moves.
//!
//! **The one rule.** No accessor may walk the graph. Everything below is a
//! field read, a hash lookup, or a slice index, so a per-widget call is
//! O(1) and nothing has to be rebuilt when the document moves.

pub(crate) mod input_ctx;
pub(crate) mod node_ctx;
pub(crate) mod output_ctx;

use std::collections::BTreeSet;

use palantir::Display;
use scenarium::{DataType, Graph, InputPort, Library, NodeId, OutputPort, Subscription};

use crate::core::document::Document;
use crate::core::document::graph_view::{GraphView, StackedNode};
use crate::core::document::item_id::ItemId;
use crate::core::document::viewport::Viewport;
use crate::core::open_document::OpenDocument;
use crate::gui::frame::app_ctx::AppCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::port_ref::{PortKind, PortRef};
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// The frame's world and the open document it shows: what every surface at window level and below
/// reads. `Copy` (shared refs and one small value), so it threads through the draw chain like
/// `DrawCtx`.
///
/// It takes the [`OpenDocument`] rather than the bare [`Document`] because the window shows both
/// halves of it: the graph and its pane arrangement, and the unsaved-changes flag, a fact about the
/// document *and* the file behind it. Read-only, so nothing here can reach the edit history the pair
/// also holds.
///
/// **Composed per phase, never per frame.** The document reference cannot outlive the phase that
/// took it: the session drains the frame's requests into the document between phases, and that
/// mutation needs the document exclusively. So one of these is built at each call into
/// [`MainWindow`](crate::gui::main_window::MainWindow), over the document as it stands at that
/// instant.
///
/// A widget reaches the theme, the library and the last run through the same context it asks about
/// nodes, one path to each. Refused: a window level above this one, whose only addition below it
/// was the camera.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GraphCtx<'a> {
    /// The frame's read-only world: the theme every widget paints from, the library each node's
    /// declaration resolves through, and the last run's per-node verdicts.
    app: AppCtx<'a>,
    /// The document this frame shows, and the file behind it.
    open: &'a OpenDocument,
    /// The document's camera as this frame's display draws it. See [`Self::camera`].
    camera: Viewport,
}

impl<'a> GraphCtx<'a> {
    /// The context of `open` under the frame's `app` on `display`.
    pub(crate) fn new(app: AppCtx<'a>, open: &'a OpenDocument, display: Display) -> Self {
        Self {
            camera: open.document().main_view.viewport.landed(display),
            app,
            open,
        }
    }

    /// The open document whole: its content *and* the file it came from. For the tab strip, which
    /// also shows [`OpenDocument::is_dirty`].
    pub(crate) const fn open(self) -> &'a OpenDocument {
        self.open
    }

    /// The whole document behind this context.
    ///
    /// For the readers that build an intent against more of it than the
    /// shown graph — a duplicate copies wiring the projection alone can't
    /// describe. Prefer [`Self::body`] / [`Self::view`], which say which
    /// half is being read.
    pub(crate) const fn document(self) -> &'a Document {
        self.open.document()
    }

    /// The authoring graph this pane shows.
    pub(crate) const fn body(self) -> &'a Graph {
        &self.document().graph
    }

    /// Its view metadata: placements, viewport, committed selection.
    pub(crate) const fn view(self) -> &'a GraphView {
        &self.document().main_view
    }

    /// The camera the canvas draws and hit-tests through: the stored one
    /// [landed](Viewport::landed) on this frame's display. Every reader of the canvas's space takes
    /// this one; only the gesture that moves the camera reads the exact [`GraphView::viewport`].
    pub(crate) const fn camera(self) -> Viewport {
        self.camera
    }

    /// The palette and metrics every widget in this pane paints from.
    pub(crate) const fn theme(self) -> &'a Theme {
        self.app.theme()
    }

    /// The library every node's declaration is resolved through — for the
    /// readers that need type metadata a port doesn't carry (an enum's
    /// registered variants, a type's display name).
    pub(crate) const fn library(self) -> &'a Library {
        self.app.library()
    }

    /// The last run's results, for the readers that want more of a node than
    /// its [`NodeCtx`] surfaces — its logs, its failure message, the value
    /// a preview published.
    pub(crate) const fn run_state(self) -> &'a RunState {
        self.app.run_state()
    }

    /// The declared type of `port`, read straight off the graph and the
    /// library. For a per-wire reader that has no node context and wants none.
    ///
    /// # Panics
    ///
    /// If the graph does not hold `port`: a committed wire joins two declared
    /// ports, and an in-flight wire drops once its start is gone.
    pub(crate) fn data_type(self, port: PortRef) -> &'a DataType {
        let func = self
            .body()
            .find(port.node_id)
            .expect("a painted port's node is in the graph")
            .func(self.library());
        match port.kind {
            PortKind::Input => &func.inputs()[port.port_idx].data_type,
            PortKind::Output => &func.outputs()[port.port_idx].data_type,
        }
    }

    /// This pane's placed nodes — every graph node and every stub — in no
    /// particular order.
    ///
    /// Driven by the view's placements, which name exactly those nodes, since
    /// a placement carries the position a node is drawn at.
    ///
    /// Unordered because almost nothing needs the stack: scanning for
    /// emitters, resolving a drag anchor, framing the viewport and hit-testing
    /// a rubber band all want the set. The one pass that draws asks for
    /// [`Self::paint_order`] and pays for the sort there.
    pub(crate) fn nodes(self) -> impl Iterator<Item = NodeCtx<'a>> {
        self.view()
            .node_placements
            .iter()
            .map(move |(id, placement)| NodeCtx::resolve(self, *id, placement.pos))
    }

    /// This graph's node ids back-to-front into `out`: later entries draw in
    /// front, and `GraphIntent::Raise` lifts one past the rest. Resolve each
    /// with [`Self::node`].
    ///
    /// Ids rather than resolved nodes because the sort needs a buffer and a
    /// [`NodeCtx`] borrows this context — see
    /// [`GraphView::paint_order`](crate::core::document::graph_view::GraphView::paint_order)
    /// for why the buffer is the caller's.
    pub(crate) fn paint_order(self, out: &mut Vec<StackedNode>) {
        self.view().paint_order(out);
    }

    /// One placed node — a graph node or a stub — or `None` for an id the
    /// document does not hold: one deleted since the caller read the id.
    pub(crate) fn node(self, node_id: NodeId) -> Option<NodeCtx<'a>> {
        let placement = *self.view().node_placements.get(&node_id)?;
        Some(NodeCtx::resolve(self, node_id, placement.pos))
    }

    /// Whether the document places `node_id`, as a graph node or a stub.
    pub(crate) fn contains(self, node_id: NodeId) -> bool {
        self.view().node_placements.contains_key(&node_id)
    }

    /// This graph's wires, as `(consumer input ← producer output)`.
    pub(crate) fn wires(self) -> impl Iterator<Item = (InputPort, OutputPort)> + 'a {
        self.body().wires()
    }

    /// This graph's event-subscription edges.
    pub(crate) fn subscriptions(self) -> impl Iterator<Item = Subscription> + 'a {
        self.body().subscriptions()
    }

    /// This graph's committed selection.
    pub(crate) const fn selected(self) -> &'a BTreeSet<ItemId> {
        &self.view().selected
    }

    /// Whether `item` is in this graph's committed selection.
    pub(crate) fn is_selected(self, item: ItemId) -> bool {
        self.view().selected.contains(&item)
    }
}

#[cfg(test)]
pub(crate) mod internals;

#[cfg(test)]
mod tests;
