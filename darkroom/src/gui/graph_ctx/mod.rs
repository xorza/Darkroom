//! The graph as the UI reads it: the document, resolved against the library
//! and the last run.
//!
//! Nothing here is copied or cached. A [`GraphCtx`] is the frame's [`DocCtx`] narrowed to one
//! graph of the open document; the handles it hands out ([`NodeCtx`], [`InputCtx`](input_ctx::InputCtx),
//! [`OutputCtx`](output_ctx::OutputCtx)) each resolve one more borrow and answer from the
//! authority that owns the fact — the node's record off the document, its ports off the library's
//! declaration, its status off the run. A widget therefore cannot read anything a frame behind.
//!
//! **The one rule.** No accessor may walk the graph. Everything below is a
//! field read, a hash lookup, or a slice index, so a per-widget call is
//! O(1) and nothing has to be rebuilt when the document moves.

pub(crate) mod ghost_port;
pub(crate) mod input_ctx;
pub(crate) mod node_ctx;
pub(crate) mod output_ctx;

use std::collections::BTreeSet;

use palantir::{Display, RgbaF32};
use scenarium::{
    DataType, Graph, GraphId, InputPort, InstancePath, Library, NodeId, OutputPort, ProgramId,
    SavedPort, Scope, Subscription,
};

use crate::core::document::Document;
use crate::core::document::graph_view::{GraphView, StackedNode};
use crate::core::document::item_id::ItemId;
use crate::core::document::viewport::Viewport;
use crate::core::open_document::OpenDocument;
use crate::core::shared_library::SharedLibrary;
use crate::gui::frame::doc_ctx::DocCtx;
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::port_ref::{PortKind, PortRef};
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// One graph pane's world: the frame's [`DocCtx`] and one graph of its document, with that
/// graph's view, the instance its run results are read through and the camera this frame draws
/// it through. What every surface of a graph pane
/// and below reads. `Copy` (shared refs and small values), so it threads through the draw chain
/// like `DrawCtx`.
///
/// **Composed per phase, never per frame**, as its [`DocCtx`] is: built at each call into a pane,
/// over the document as it stands at that instant.
///
/// A widget reaches the theme, the library and the last run through the same context it asks about
/// nodes, one path to each.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GraphCtx<'a> {
    doc: DocCtx<'a>,
    graph: GraphId,
    body: &'a Graph,
    view: &'a GraphView,
    /// See [`Self::through`]. Borrowed, as the context is copied into every node's and a path
    /// is 113 bytes.
    through: Option<&'a InstancePath>,
    /// The view's camera as this frame's display draws it. See [`Self::camera`].
    camera: Viewport,
}

impl<'a> GraphCtx<'a> {
    /// The context of `graph` of `doc`'s document, drawn on `display`.
    ///
    /// # Panics
    /// If the document holds no `graph`, or no view of it.
    pub(crate) fn of(doc: DocCtx<'a>, graph: GraphId, display: Display) -> Self {
        let document = doc.document();
        let view = document.view(graph);
        Self {
            doc,
            graph,
            body: document.graphs.graph(graph),
            view,
            through: document.through(graph),
            camera: view.viewport.landed(display),
        }
    }

    /// Which graph of the document this pane shows.
    pub(crate) const fn graph(self) -> GraphId {
        self.graph
    }

    /// The whole document behind this context, for the readers that build an intent against
    /// more of it than the shown graph. Prefer [`Self::body`] / [`Self::view`], which say which
    /// half is being read.
    pub(crate) const fn document(self) -> &'a Document {
        self.doc.document()
    }

    /// The open document behind this context: its history and revisions beside the document.
    pub(crate) const fn open(self) -> &'a OpenDocument {
        self.doc.open()
    }

    /// The authoring graph this pane shows.
    pub(crate) const fn body(self) -> &'a Graph {
        self.body
    }

    /// Its view metadata: placements, viewport, committed selection.
    pub(crate) const fn view(self) -> &'a GraphView {
        self.view
    }

    /// The camera the canvas draws and hit-tests through: the stored one
    /// [landed](Viewport::landed) on this frame's display. Every reader of the canvas's space takes
    /// this one; only the gesture that moves the camera reads the exact [`GraphView::viewport`].
    pub(crate) const fn camera(self) -> Viewport {
        self.camera
    }

    /// The instance whose run results this pane shows: the main graph's own path, or the one a
    /// subgraph's side names; `None` for a subgraph shown through none, whose nodes show no run.
    pub(crate) const fn through(self) -> Option<InstancePath> {
        self.through.copied()
    }

    /// This context read through `through` in place of the instance the graph's pane shows: a
    /// pinned inspector's, which keeps the instance it was pinned to.
    pub(crate) const fn through_instance<'b>(
        self,
        through: Option<&'b InstancePath>,
    ) -> GraphCtx<'b>
    where
        'a: 'b,
    {
        let narrowed: GraphCtx<'b> = self;
        GraphCtx {
            through,
            ..narrowed
        }
    }

    /// The program node `node_id` of this graph is in the instance [`Self::through`] names, or
    /// `None` when the pane shows no instance.
    pub(crate) fn program_id(self, node_id: NodeId) -> Option<ProgramId> {
        self.through.map(|path| path.node(node_id))
    }

    /// The palette and metrics every widget in this pane paints from.
    pub(crate) const fn theme(self) -> &'a Theme {
        self.doc.theme()
    }

    /// The library every node's declaration is resolved through — for the
    /// readers that need type metadata a port doesn't carry (an enum's
    /// registered variants, a type's display name).
    pub(crate) const fn library(self) -> &'a Library {
        self.doc.library()
    }

    /// What the nodes of the graph this pane shows resolve against.
    pub(crate) fn scope(self) -> Scope<'a> {
        self.document().graphs.scope(self.library(), self.graph)
    }

    /// The last run's results, for the readers that want more of a node than
    /// its [`NodeCtx`] surfaces — its logs, its failure message, the value
    /// a preview published.
    pub(crate) const fn run_state(self) -> &'a RunState {
        self.doc.run_state()
    }

    /// The library folder's shared subgraphs, as the last scan read them.
    pub(crate) const fn shared(self) -> &'a SharedLibrary {
        self.doc.shared()
    }

    /// The color a port of `data_type` paints at rest, and the wires touching it: its type's hue
    /// ([`Theme::port_color`]), or the neutral untyped color for a type the library does not
    /// register, which a subgraph's interface keeps as written.
    pub(crate) fn port_color(self, data_type: DataType) -> RgbaF32 {
        let theme = self.theme();
        if self.library().knows(data_type) {
            theme.port_color(data_type)
        } else {
            theme.port.input
        }
    }

    /// The type one end of a ghost wire has or had, as far as anything knows
    /// ([`Graph::shown_input`]): `saved` is the port the ghost holds for that end. `None` paints
    /// the neutral untyped color.
    pub(crate) fn ghost_end_type(
        self,
        port: PortRef,
        saved: Option<&SavedPort>,
    ) -> Option<DataType> {
        let (body, scope) = (self.body(), self.scope());
        let shown = match port.kind {
            PortKind::Input => {
                body.shown_input(scope, InputPort::new(port.node_id, port.port), saved)
            }
            PortKind::Output => {
                body.shown_output(scope, OutputPort::new(port.node_id, port.port), saved)
            }
        };
        shown.map(|shown| shown.data_type)
    }

    /// The color a ghost port of `data_type` and the ghost wires on it paint: its type's, or the
    /// neutral untyped color where nothing kept the type.
    pub(crate) fn ghost_color(self, data_type: Option<DataType>) -> RgbaF32 {
        match data_type {
            Some(data_type) => self.port_color(data_type),
            None => self.theme().port.input,
        }
    }

    /// The declared type of `port`, read straight off the graph and the
    /// library. For a per-wire reader that has no node context and wants none.
    ///
    /// # Panics
    ///
    /// If the graph does not hold `port`: a committed wire joins two declared
    /// ports, and an in-flight wire drops once its start is gone.
    pub(crate) fn data_type(self, port: PortRef) -> DataType {
        let decl = self
            .body()
            .find(port.node_id)
            .expect("a painted port's node is in the graph")
            .decl(self.scope());
        match port.kind {
            PortKind::Input => {
                decl.ports()
                    .input(port.port)
                    .expect("a painted port is declared")
                    .data_type
            }
            PortKind::Output => {
                decl.ports()
                    .output(port.port)
                    .expect("a painted port is declared")
                    .data_type
            }
        }
    }

    /// This pane's placed nodes — every graph node and every ghost node — in no
    /// particular order.
    ///
    /// Driven by the view's placements. Unordered because almost nothing needs the stack; the
    /// one pass that draws asks for [`Self::paint_order`] and pays for the sort there.
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

    /// One placed node — a graph node or a ghost node — or `None` for an id the
    /// document does not hold: one deleted since the caller read the id.
    pub(crate) fn node(self, node_id: NodeId) -> Option<NodeCtx<'a>> {
        let placement = *self.view().node_placements.get(&node_id)?;
        Some(NodeCtx::resolve(self, node_id, placement.pos))
    }

    /// Whether the document places `node_id`, as a graph node or a ghost node.
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
