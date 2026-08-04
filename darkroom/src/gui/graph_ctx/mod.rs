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
//! field read, a hash lookup, or a slice index, so a per-widget call costs
//! what the projection this replaced used to charge per frame. The one answer
//! that cannot come off a declaration — a wildcard output's resolved type —
//! is read out of the [`OutputTypes`] table the context carries, resolved once
//! by [`GraphCtx::new`] rather than per read (see
//! [`OutputCtx::ty`](output_ctx::OutputCtx::ty)).

pub(crate) mod input_ctx;
pub(crate) mod node_ctx;
pub(crate) mod output_ctx;

use std::collections::BTreeSet;

use scenarium::{Graph, InputPort, Library, NodeId, OutputPort, OutputTypes, Subscription};

use crate::core::document::{Document, GraphView, Viewport};
use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;
use crate::gui::window::ctx::WindowCtx;

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
    /// Resolved by [`Self::new`] against the document the `window` carries, and
    /// exclusively borrowed for as long as this context lives — so it cannot be
    /// a graph edit behind, and nothing can move it out from under a reader.
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
    /// **Resolves `output_types` against that document on the way in**, which
    /// is why it arrives `&mut` and leaves shared. A context's readers answer a
    /// wildcard port off that table, so its freshness is not a contract for
    /// callers to keep — composing a context *is* the refresh, and the borrow
    /// then lasts as long as the context, so nothing can edit the graph out from
    /// under it. Darkroom edits between passes and composes a context per pass,
    /// so each pass pays one resolve over the document it was handed.
    ///
    /// The table is threaded in rather than owned because the context is `Copy`:
    /// the caller keeps the allocation across frames, and a refresh reuses its
    /// capacity instead of building a map per pass.
    pub(crate) fn new(window: WindowCtx<'a>, output_types: &'a mut OutputTypes) -> Self {
        let doc = window.document();
        output_types.update(&doc.graph, window.app().library());
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
    pub(crate) fn is_visible(self) -> bool {
        self.is_visible
    }

    /// The whole document behind this context.
    ///
    /// For the readers that build an intent against more of it than the
    /// shown graph — a duplicate copies wiring the projection alone can't
    /// describe. Prefer [`Self::body`] / [`Self::view`], which say which
    /// half is being read.
    pub(crate) fn document(self) -> &'a Document {
        self.window.document()
    }

    /// The authoring graph this pane shows.
    pub(crate) fn body(self) -> &'a Graph {
        &self.document().graph
    }

    /// Its view metadata: placements, viewport, committed selection.
    pub(crate) fn view(self) -> &'a GraphView {
        &self.document().main_view
    }

    pub(crate) fn viewport(self) -> Viewport {
        self.view().viewport
    }

    /// The palette and metrics every widget in this pane paints from.
    pub(crate) fn theme(self) -> &'a Theme {
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
    pub(crate) fn run_state(self) -> &'a RunState {
        self.window.app().run_state()
    }

    /// This graph's resolved output types. `pub(super)` because the one
    /// reader is [`OutputCtx::ty`](output_ctx::OutputCtx::ty) — a widget
    /// asks a port for its type, never the table for a port.
    pub(super) fn output_types(self) -> &'a OutputTypes {
        self.output_types
    }

    /// This graph's nodes, in no particular order.
    ///
    /// Driven by the view's placements rather than the graph's own node list,
    /// since only a placed node has somewhere to be. A placement whose node is
    /// gone is skipped rather than faked.
    ///
    /// Unordered because almost nothing needs the stack: scanning for
    /// emitters, resolving a drag anchor, framing the viewport and hit-testing
    /// a rubber band all want the set. The one pass that draws asks for
    /// [`Self::nodes_in_paint_order`] and pays for the sort there.
    pub(crate) fn nodes(self) -> impl Iterator<Item = NodeCtx<'a>> {
        self.view()
            .item_placements
            .iter()
            .filter_map(move |(id, placement)| NodeCtx::resolve(self, *id, placement.pos))
    }

    /// This graph's nodes back-to-front: later entries draw in front, and
    /// `GraphIntent::Raise` lifts one past the rest.
    ///
    /// Allocates the sorted run once, so a paint pass walks it instead of
    /// re-resolving stacking per node.
    pub(crate) fn nodes_in_paint_order(self) -> impl Iterator<Item = NodeCtx<'a>> {
        self.view()
            .paint_order()
            .into_iter()
            .filter_map(move |(id, placement)| NodeCtx::resolve(self, id, placement.pos))
    }

    /// One node of this graph, or `None` for an id it does not hold — a node
    /// deleted since the caller read the id, or one belonging to another pane.
    pub(crate) fn node(self, node_id: NodeId) -> Option<NodeCtx<'a>> {
        let placement = *self.view().item_placements.get(&node_id)?;
        NodeCtx::resolve(self, node_id, placement.pos)
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
    pub(crate) fn selected(self) -> &'a BTreeSet<NodeId> {
        &self.view().selected
    }

    /// Whether `key` is in this graph's committed selection.
    pub(crate) fn is_selected(self, key: NodeId) -> bool {
        self.view().selected.contains(&key)
    }
}

#[cfg(test)]
pub(crate) mod harness;

#[cfg(test)]
mod tests;
