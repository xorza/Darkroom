//! The graph as the UI reads it: the document, resolved against the library
//! and the last run.
//!
//! Nothing here is copied or cached. A [`GraphCtx`] is the frame's
//! [`WindowCtx`] plus one shared reference; the handles it hands out
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
//! O(1) and nothing has to be rebuilt when the document moves. The one answer
//! that cannot come off a declaration — a wildcard output's resolved type —
//! is read out of the [`OutputTypes`] table the context carries, which the
//! caller resolves once per graph edit rather than per read (see
//! [`OutputCtx::ty`](output_ctx::OutputCtx::ty)).

pub(crate) mod input_ctx;
pub(crate) mod node_ctx;
pub(crate) mod output_ctx;
pub(crate) mod output_type_cache;

use std::collections::BTreeSet;

use scenarium::{
    DataType, Graph, InputPort, Library, NodeId, OutputPort, OutputTypes, Subscription,
};

use crate::core::document::{Document, GraphView, PortKind, PortRef, StackedItem, Viewport};
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;
use crate::gui::window::window_ctx::WindowCtx;

/// The graph pane for this frame. `Copy` (the window context plus one shared
/// ref), so it threads through the draw chain like `DrawCtx`.
///
/// The canvas level of the context chain: it carries the frame's
/// [`WindowCtx`] rather than restating the refs inside it, so a widget
/// reaches the theme, the library and the last run through the same context it
/// asks about nodes — one path to each, and nothing under the canvas has to
/// name the app or window level at all.
///
/// Composing one always succeeds: the document, the library and the run are
/// there whether or not a pane happens to be showing the graph. Whether one
/// is rides along as [`Self::is_visible`], because only a single pass cares —
/// the hit sweep, which runs before the tab set settles. Every other reader is
/// reached only from a pane that is drawing, and the two entry points that
/// bridge the two worlds say so with a `debug_assert!` rather than making
/// every call site unwrap.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GraphCtx<'a> {
    /// The window's context, one level up: the document this pane's graph
    /// lives in, plus the frame's read-only world — the theme every widget
    /// paints from, the library each node's declaration resolves through (a
    /// node whose func it no longer holds reads as a
    /// [`missing`](NodeCtx::missing) stub rather than vanishing), and the
    /// last run's per-node verdicts — status, retained RAM, unfed inputs, and
    /// the compiled program's word on what is a sink.
    window: WindowCtx<'a>,
    /// Every output port's *resolved* type — the wildcard chains followed
    /// once for the whole graph, so reading one is a lookup rather than a
    /// walk. See [`OutputCtx::ty`](output_ctx::OutputCtx::ty).
    ///
    /// Resolved by the caller against the document the `window` carries, and
    /// borrowed for as long as this context lives — so nothing can move it out
    /// from under a reader.
    output_types: &'a OutputTypes,
    /// Whether a pane is showing this graph, snapshot when the context was
    /// composed. See [`Self::is_visible`].
    is_visible: bool,
}

impl<'a> GraphCtx<'a> {
    /// Derive the graph pane's context from the `window`'s — the frame's
    /// read-only world and the document it is showing.
    ///
    /// Visibility is asked of the *document*, not of its contents: a graph
    /// with no nodes on an active tab is a legitimate pane, and one that
    /// answered "no nodes, so no pane" would leave a fresh document with no
    /// canvas to place its first node on.
    ///
    /// `output_types` must be resolved against that document — see
    /// [`OutputTypeCache`](output_type_cache::OutputTypeCache), which resolves
    /// it again only after an edit that can retype an output. The table is
    /// threaded in rather than owned because the context is `Copy`.
    pub(crate) fn new(window: WindowCtx<'a>, output_types: &'a OutputTypes) -> Self {
        let doc = window.document();
        Self {
            is_visible: doc.shows_graph(),
            window,
            output_types,
        }
    }

    /// Whether a pane is showing this graph.
    ///
    /// Resolved once when the context was composed rather than asked of the
    /// layout per call, so it obeys the module's one rule: every accessor is a
    /// field read. The hit sweep is the one reader — it runs at the top of the
    /// frame, before the navigation phase settles which tabs are active, so it
    /// cannot assume a canvas.
    pub(crate) const fn is_visible(self) -> bool {
        self.is_visible
    }

    /// The whole document behind this context.
    ///
    /// For the readers that build an intent against more of it than the
    /// shown graph — a duplicate copies wiring the projection alone can't
    /// describe. Prefer [`Self::body`] / [`Self::view`], which say which
    /// half is being read.
    pub(crate) const fn document(self) -> &'a Document {
        self.window.document()
    }

    /// The authoring graph this pane shows.
    pub(crate) const fn body(self) -> &'a Graph {
        &self.document().graph
    }

    /// Its view metadata: placements, viewport, committed selection.
    pub(crate) const fn view(self) -> &'a GraphView {
        &self.document().main_view
    }

    pub(crate) const fn viewport(self) -> Viewport {
        self.view().viewport
    }

    /// The palette and metrics every widget in this pane paints from.
    pub(crate) const fn theme(self) -> &'a Theme {
        self.window.app().theme()
    }

    /// The library every node's declaration is resolved through — for the
    /// readers that need type metadata a port doesn't carry (an enum's
    /// registered variants, a type's display name).
    pub(crate) fn library(self) -> &'a Library {
        self.window.app().library()
    }

    /// The last run's results, for the readers that want more of a node than
    /// its [`NodeCtx`] surfaces — its logs, its failure message, the value
    /// a preview published.
    pub(crate) const fn run_state(self) -> &'a RunState {
        self.window.app().run_state()
    }

    /// This graph's resolved output types. `pub(super)` because the one
    /// reader is [`OutputCtx::ty`](output_ctx::OutputCtx::ty) — a widget
    /// asks a port for its type, never the table for a port.
    pub(super) const fn output_types(self) -> &'a OutputTypes {
        self.output_types
    }

    /// The type of `port` — an input's declared type, or an output's resolved
    /// one — read straight off the graph, the library and the type table, or
    /// `None` for a port this graph does not hold. For a per-wire reader that
    /// has no node context and wants none.
    pub(crate) fn port_type(self, port: PortRef) -> Option<&'a DataType> {
        match port.kind {
            PortKind::Input => self
                .body()
                .find(port.node_id)?
                .func(self.library())?
                .inputs
                .get(port.port_idx)
                .map(|input| &input.data_type),
            PortKind::Output => self
                .output_types
                .get(OutputPort::new(port.node_id, port.port_idx)),
        }
    }

    /// This graph's nodes, in no particular order.
    ///
    /// Driven by the view's placements, which name exactly the graph's nodes,
    /// since a placement carries the position a node is drawn at.
    ///
    /// Unordered because almost nothing needs the stack: scanning for
    /// emitters, resolving a drag anchor, framing the viewport and hit-testing
    /// a rubber band all want the set. The one pass that draws asks for
    /// [`Self::paint_order`] and pays for the sort there.
    pub(crate) fn nodes(self) -> impl Iterator<Item = NodeCtx<'a>> {
        self.view()
            .item_placements
            .iter()
            .map(move |(id, placement)| NodeCtx::resolve(self, *id, placement.pos))
    }

    /// This graph's node ids back-to-front into `out`: later entries draw in
    /// front, and `GraphIntent::Raise` lifts one past the rest. Resolve each
    /// with [`Self::node`].
    ///
    /// Ids rather than resolved nodes because the sort needs a buffer and a
    /// [`NodeCtx`] borrows this context — see
    /// [`GraphView::paint_order`](crate::core::document::GraphView::paint_order)
    /// for why the buffer is the caller's.
    pub(crate) fn paint_order(self, out: &mut Vec<StackedItem>) {
        self.view().paint_order(out);
    }

    /// One node of this graph, or `None` for an id it does not hold — a node
    /// deleted since the caller read the id.
    pub(crate) fn node(self, node_id: NodeId) -> Option<NodeCtx<'a>> {
        let placement = *self.view().item_placements.get(&node_id)?;
        Some(NodeCtx::resolve(self, node_id, placement.pos))
    }

    pub(crate) fn contains(self, node_id: NodeId) -> bool {
        self.node(node_id).is_some()
    }

    /// This graph's data edges, as `(consumer input ← producer output)`.
    pub(crate) fn connections(self) -> impl Iterator<Item = (InputPort, OutputPort)> + 'a {
        self.body().edges()
    }

    /// This graph's event-subscription edges.
    pub(crate) fn subscriptions(self) -> impl Iterator<Item = Subscription> + 'a {
        self.body().subscriptions()
    }

    /// This graph's committed selection.
    pub(crate) const fn selected(self) -> &'a BTreeSet<NodeId> {
        &self.view().selected
    }

    /// Whether `key` is in this graph's committed selection.
    pub(crate) fn is_selected(self, key: NodeId) -> bool {
        self.view().selected.contains(&key)
    }
}

#[cfg(test)]
pub(crate) mod internals;

#[cfg(test)]
mod tests;
