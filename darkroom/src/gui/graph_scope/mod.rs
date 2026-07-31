//! The graph as the UI reads it: the document, resolved against the library
//! and the last run.
//!
//! Nothing here is copied or cached. A [`GraphScope`] is three shared
//! references; the handles it hands out ([`NodeScope`], [`InputScope`],
//! [`OutputScope`]) each resolve one more borrow and answer every question
//! from the authority that owns it — the node's record off the document, its
//! ports off the library's declaration, its status off the run. A widget
//! therefore cannot read anything a frame behind, and there is nothing to
//! invalidate when the document moves.
//!
//! **The one rule.** No accessor may walk the graph. Everything below is a
//! field read, a hash lookup, or a slice index, so a per-widget call costs
//! what the projection this replaced used to charge per frame. The one answer
//! that cannot come off a declaration — a wildcard output's resolved type —
//! is read out of the [`OutputTypes`] table the scope carries, resolved once
//! by [`GraphScope::for_document`] rather than per read (see
//! [`OutputScope::ty`](output_scope::OutputScope::ty)).

pub(crate) mod input_scope;
pub(crate) mod node_scope;
pub(crate) mod output_scope;

use std::collections::BTreeSet;

use scenarium::{Graph, InputPort, Library, NodeId, OutputPort, OutputTypes, Subscription};

use crate::core::document::{Document, GraphView, Viewport};
use crate::gui::graph_scope::node_scope::NodeScope;
use crate::gui::run_state::RunState;

#[cfg(test)]
mod tests;

/// The graph pane for this frame. `Copy` (four shared refs), so it threads
/// through the draw chain like `RecordCtx`.
///
/// Holding one is the proof that a pane *is* showing the graph:
/// [`Self::for_document`] is the only way to obtain one and it checks that
/// once, so no reader has to. A pass that runs whether or not a graph is on
/// screen takes `Option<GraphScope>` and says so in its signature.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GraphScope<'a> {
    doc: &'a Document,
    /// Resolves each node's declaration — its ports, their types and
    /// defaults. A node whose func the library no longer holds reads as a
    /// [`missing`](NodeScope::missing) stub rather than vanishing.
    library: &'a Library,
    /// Last run's per-node verdicts: status, retained RAM, unfed inputs, and
    /// the compiled program's word on what is a sink.
    run_state: &'a RunState,
    /// Every output port's *resolved* type — the wildcard chains followed
    /// once for the whole graph, so reading one is a lookup rather than a
    /// walk. See [`OutputScope::ty`](output_scope::OutputScope::ty).
    ///
    /// Resolved by [`Self::for_document`] against the `doc` beside it, and
    /// exclusively borrowed for as long as this scope lives — so it cannot be
    /// a graph edit behind, and nothing can move it out from under a reader.
    output_types: &'a OutputTypes,
}

impl<'a> GraphScope<'a> {
    /// The scope over `doc`'s graph, or `None` when no pane is showing it.
    ///
    /// Asked of the *document*: a graph with no nodes on an active tab is a
    /// legitimate pane, and one that answered "no nodes, so no pane" would
    /// leave a fresh document with no canvas to place its first node on.
    ///
    /// **Resolves `output_types` against `doc` on the way in**, which is why
    /// it arrives `&mut` and leaves shared. A scope's readers answer a
    /// wildcard port off that table, so its freshness is not a contract for
    /// callers to keep — composing a scope *is* the refresh, and the borrow
    /// then lasts as long as the scope, so nothing can edit the graph out from
    /// under it. Darkroom edits between passes and composes a scope per pass,
    /// so each pass pays one resolve over the document it was handed.
    ///
    /// The table is threaded in rather than owned because the scope is `Copy`:
    /// the caller keeps the allocation across frames, and a refresh reuses its
    /// capacity instead of building a map per pass.
    pub(crate) fn for_document(
        doc: &'a Document,
        library: &'a Library,
        run_state: &'a RunState,
        output_types: &'a mut OutputTypes,
    ) -> Option<Self> {
        output_types.update(&doc.graph, library);
        doc.shows_graph().then_some(Self {
            doc,
            library,
            run_state,
            output_types,
        })
    }

    /// The whole document behind this scope.
    ///
    /// For the readers that build an intent against more of it than the
    /// shown graph — a duplicate copies wiring the projection alone can't
    /// describe. Prefer [`Self::body`] / [`Self::view`], which say which
    /// half is being read.
    pub(crate) fn document(self) -> &'a Document {
        self.doc
    }

    /// The authoring graph this pane shows.
    pub(crate) fn body(self) -> &'a Graph {
        &self.doc.graph
    }

    /// Its view metadata: placements, viewport, committed selection.
    pub(crate) fn view(self) -> &'a GraphView {
        &self.doc.main_view
    }

    pub(crate) fn viewport(self) -> Viewport {
        self.doc.main_view.viewport
    }

    /// The library every node's declaration is resolved through — for the
    /// readers that need type metadata a port doesn't carry (an enum's
    /// registered variants, a type's display name).
    pub(crate) fn library(self) -> &'a Library {
        self.library
    }

    /// The last run's results, for the readers that want more of a node than
    /// its [`NodeScope`] surfaces — its logs, its failure message, the value
    /// a preview published.
    pub(crate) fn run_state(self) -> &'a RunState {
        self.run_state
    }

    /// This graph's resolved output types. `pub(super)` because the one
    /// reader is [`OutputScope::ty`](output_scope::OutputScope::ty) — a widget
    /// asks a port for its type, never the table for a port.
    pub(super) fn output_types(self) -> &'a OutputTypes {
        self.output_types
    }

    /// This graph's nodes, in paint order: later entries draw in front, and
    /// `GraphIntent::Raise` moves one to the end.
    ///
    /// Driven by the view's placements rather than the graph's own node list,
    /// because the paint stack *is* that order. A placement whose node is
    /// gone is skipped rather than faked.
    pub(crate) fn nodes(self) -> impl Iterator<Item = NodeScope<'a>> {
        self.view()
            .item_placements
            .iter()
            .filter_map(move |(id, pos)| NodeScope::resolve(self, *id, *pos))
    }

    /// One node of this graph, or `None` for an id it does not hold — a node
    /// deleted since the caller read the id, or one belonging to another pane.
    pub(crate) fn node(self, node_id: NodeId) -> Option<NodeScope<'a>> {
        let pos = *self.view().item_placements.get(&node_id)?;
        NodeScope::resolve(self, node_id, pos)
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
pub(crate) mod internals {
    use glam::Vec2;
    use scenarium::{FuncId, Library, Node, NodeId, NodeKind, OutputTypes};

    use crate::core::document::Document;
    use crate::gui::graph_scope::GraphScope;
    use crate::gui::run_state::RunState;

    /// The four sources a [`GraphScope`] composes, owned together so a test
    /// can hand one out. Every canvas test that needs a scope builds on this.
    ///
    /// The output-type table starts empty: [`Self::scope`] resolves it, the
    /// same way composing a scope does anywhere else — which is why that
    /// method takes `&mut self`.
    #[derive(Debug, Default)]
    pub(crate) struct ScopeFixture {
        pub(crate) doc: Document,
        pub(crate) library: Library,
        pub(crate) run_state: RunState,
        output_types: OutputTypes,
    }

    impl ScopeFixture {
        pub(crate) fn over(doc: Document, library: Library) -> Self {
            Self {
                doc,
                library,
                ..Self::default()
            }
        }

        /// Nodes placed at the given positions, each from a func no library
        /// holds — so they resolve as portless stubs. Enough for the tests
        /// that read only a node's identity and where it sits.
        pub(crate) fn with_nodes(nodes: impl IntoIterator<Item = (NodeId, Vec2)>) -> Self {
            let mut doc = Document::default();
            for (node_id, pos) in nodes {
                doc.graph
                    .insert(node_id, Node::new(NodeKind::Func(FuncId::unique())));
                doc.main_view.item_placements.insert(node_id, pos);
            }
            Self::over(doc, Library::default())
        }

        /// Give the graph a committed selection.
        pub(crate) fn with_selection(mut self, selected: impl IntoIterator<Item = NodeId>) -> Self {
            self.doc.main_view.selected.extend(selected);
            self
        }

        pub(crate) fn scope(&mut self) -> GraphScope<'_> {
            GraphScope::for_document(
                &self.doc,
                &self.library,
                &self.run_state,
                &mut self.output_types,
            )
            .expect("the fixture's document shows the graph")
        }
    }
}
