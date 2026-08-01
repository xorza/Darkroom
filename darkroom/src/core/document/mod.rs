pub(crate) mod dock;
pub(crate) mod open_document;
mod serde;
pub(crate) mod validate;

use ::serde::{Deserialize, Serialize};
use glam::Vec2;
use indexmap::IndexMap;
use scenarium::{DetachedNode, Graph as CoreGraph, InputPort, Node, NodeId, NodeKind, OutputPort};
use std::collections::BTreeSet;

use crate::core::document::dock::{DockLayout, DockOp};
use crate::core::preview;

/// Whether a port consumes a binding (`Input`) or produces a value
/// (`Output`). Scoped to the data-port subset until Trigger/Event are
/// reintroduced. `Input` ports live in the left column, `Output` in
/// the right; `opposite` flips between them for snap-target tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum PortKind {
    Input,
    Output,
}

impl PortKind {
    pub(crate) fn opposite(self) -> Self {
        match self {
            PortKind::Input => PortKind::Output,
            PortKind::Output => PortKind::Input,
        }
    }
}

/// One port's identity in the graph. Domain-keyed so UI passes can derive
/// its `WidgetId` (see `crate::gui::pane::graph::node::port_row::port_circle_wid`)
/// without threading a cache, and serializable so a persisted tab
/// ([`TabRef::ImageViewer`]) can bind to it. Node ids are unique across
/// the whole document, so no graph ref is needed alongside — enforced by
/// [`Document::validate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct PortRef {
    pub(crate) node_id: NodeId,
    pub(crate) kind: PortKind,
    pub(crate) port_idx: usize,
}

impl PortRef {
    /// `node_id`'s `port_idx`th input — the left-column counterpart of the
    /// graph's [`InputPort`].
    pub(crate) fn input(node_id: NodeId, port_idx: usize) -> Self {
        Self {
            node_id,
            kind: PortKind::Input,
            port_idx,
        }
    }

    /// `node_id`'s `port_idx`th output — the right-column counterpart of the
    /// graph's [`OutputPort`].
    pub(crate) fn output(node_id: NodeId, port_idx: usize) -> Self {
        Self {
            node_id,
            kind: PortKind::Output,
            port_idx,
        }
    }
}

impl From<InputPort> for PortRef {
    fn from(port: InputPort) -> Self {
        Self::input(port.node_id, port.port_idx)
    }
}

impl From<OutputPort> for PortRef {
    fn from(port: OutputPort) -> Self {
        Self::output(port.node_id, port.port_idx)
    }
}

/// What an editor tab shows: the document's graph ([`TabRef::Graph`]), or a
/// non-graph app view like [`TabRef::Preferences`] (the settings window) or an
/// image viewer. Persisted + undoable like the rest of the tab/view state, so
/// reopening a document restores its open tabs and Ctrl+Z walks tab
/// open/close.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TabRef {
    /// The graph pane.
    Graph,
    /// The app-preferences / settings view — no graph, no canvas.
    Preferences,
    /// A full-resolution viewer of one port's runtime image — one tab per
    /// port, deduped on open. Content is runtime-only
    /// (`crate::gui::pane::viewer`): a restored tab pulls any current value
    /// from `RunState` when drawn. Pruned when its node is deleted.
    ///
    /// Keyed by the preview node whose value it shows — the same identity the
    /// preview card on the canvas uses, so the two can never disagree about
    /// which value a tab is for.
    ImageViewer(NodeId),
}

/// A graph's camera: pan offset (canvas-local px) + zoom factor. One
/// value shared by the persisted per-graph [`GraphView`], the per-frame
/// `Scene` projection, and the `SetViewport` edit, so the three can't
/// drift on field names or semantics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Viewport {
    pub(crate) pan: Vec2,
    pub(crate) zoom: f32,
}

impl Viewport {
    pub(crate) fn is_valid(self) -> bool {
        self.pan.is_finite() && self.zoom.is_finite() && self.zoom > 0.0
    }
}

impl Default for Viewport {
    /// Origin pan, 1:1 zoom.
    fn default() -> Self {
        Self {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

/// Editor-side view metadata for the document's graph: per-item positions and
/// paint order, the viewport, and the selection (`Document::main_view`). The
/// graph *data* itself lives in the core `Graph`; this is purely how the editor
/// presents and navigates it.
///
/// **Everything here is persisted and undoable, by design** — reopening
/// a file restores the exact camera and selection, and Ctrl+Z walks
/// camera/selection changes alongside structural edits (see the long
/// note that used to live on `Document`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GraphView {
    /// Every node body's position, in the canvas's **paint stack** order:
    /// later items draw in front, and `GraphIntent::Raise` lifts one to the top.
    /// Exactly one entry per graph node — enforced by [`Self::validate`].
    #[serde(with = "crate::core::document::serde")]
    pub(crate) item_placements: IndexMap<NodeId, Vec2>,
    pub(crate) viewport: Viewport,
    /// `BTreeSet` so equality and serialization are order-independent
    /// (no spurious undo entries from reordering).
    pub(crate) selected: BTreeSet<NodeId>,
}

impl PartialEq for GraphView {
    fn eq(&self, other: &Self) -> bool {
        self.viewport == other.viewport
            && self.selected == other.selected
            // `IndexMap`'s own `PartialEq` ignores order; the paint stack *is*
            // the order, so compare as sequences. `Iterator::eq` is exactly
            // that — same length, pairwise equal.
            && self.item_placements.iter().eq(other.item_placements.iter())
    }
}

impl Eq for GraphView {}

impl GraphView {
    /// A fresh view seeded with a zero-positioned item for every node in
    /// `graph`.
    pub(crate) fn for_graph(graph: &CoreGraph) -> Self {
        let mut item_placements = IndexMap::with_capacity(graph.len());
        for node in graph.iter() {
            item_placements.insert(node.id, Vec2::ZERO);
        }
        Self {
            item_placements,
            ..Default::default()
        }
    }

    pub(crate) fn move_item_to_index(&mut self, key: &NodeId, target_index: usize) {
        let from = self
            .item_placements
            .get_index_of(key)
            .expect("view item to move must exist");
        let to = target_index.min(self.item_placements.len() - 1);
        self.item_placements.move_index(from, to);
    }
}

/// The thing being edited: the authoring `Graph`, the editor view metadata
/// positioning it, and the pane layout showing it. The `Library` it resolves
/// against lives one level up on `App` (runtime-owned).
///
/// The two halves are public because a caller that touches both wants them
/// borrowed together, and destructuring is what proves they are disjoint —
/// a pair of accessor methods would each borrow the whole `Document` and
/// couldn't be held at once.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Document {
    pub(crate) graph: CoreGraph,
    pub(crate) main_view: GraphView,
    /// The pane arrangement: open tabs grouped into split panes, plus
    /// the focused group. Persisted + undoable like the rest of the view
    /// state (every layout mutation is an undoable `DockOp`).
    #[serde(default)]
    pub(crate) layout: DockLayout,
}

/// Whether the graph still holds `node_id`.
///
/// **The** liveness question, and the single definition of it. Node ids are
/// never reused, so a node absent here is gone for good — which is what makes
/// this the retention rule for every `NodeId`-keyed cache that outlives the
/// scene: the canvas geometry, the preview values, the open inspectors.
/// Absence from a *scene* means only that a node is off-screen or on a closed
/// tab; absence from here is permanent.
///
/// Free-standing over the graph rather than a [`Document`] method so
/// [`Document::reconcile_with_graph`] can ask it while the layout is borrowed
/// mutably. [`Document::holds_node`] is the `&Document` lift every other
/// caller takes, and [`tab_alive`] is the same question asked of a tab.
fn node_alive(graph: &CoreGraph, node_id: NodeId) -> bool {
    graph.find(node_id).is_some()
}

/// Whether `node` is a preview node — the narrowing
/// [`Document::holds_preview_node`] applies on top of [`node_alive`].
fn is_preview_node(node: &Node) -> bool {
    match node.kind {
        NodeKind::Func(func_id) => preview::is_preview(func_id),
        _ => false,
    }
}

/// Whether a tab still resolves against the graph: the graph pane and
/// `Preferences` always do, and a viewer tab dies with its node. The single
/// predicate behind [`Document::reconcile_with_graph`]'s fast-path *and* its
/// prune, so the two can't drift.
fn tab_alive(graph: &CoreGraph, tab: TabRef) -> bool {
    match tab {
        TabRef::Graph | TabRef::Preferences => true,
        TabRef::ImageViewer(node_id) => node_alive(graph, node_id),
    }
}

impl Document {
    /// Apply a dock op to the layout.
    ///
    /// Pane arrangement is navigation, not content: it neither records an
    /// undo step — dragging a tab back is the undo — nor flips the unsaved
    /// flag, so Ctrl+Z walks past it to the last graph edit and quitting
    /// after a rearrangement doesn't prompt. The layout still *persists*
    /// with the document; it just isn't work the exit prompt guards.
    ///
    /// No no-op filter either: an op the layout refuses (a tab that closed
    /// under the gesture, an unchanged ratio) leaves it untouched on its own.
    pub(crate) fn apply_dock_op(&mut self, op: DockOp) {
        self.layout.apply(op);
    }

    /// Drop a node from both the graph and the view (its placement and any
    /// selection membership) — the one edit that has to touch both to leave
    /// the document consistent.
    pub(crate) fn remove_node(&mut self, node_id: &NodeId) -> DetachedNode {
        self.main_view
            .item_placements
            .retain(|key, _| *key != *node_id);
        let detached = self.graph.detach_node(*node_id);
        self.main_view.selected.retain(|k| *k != *node_id);
        detached
    }

    /// Whether the graph is on a canvas — i.e. some pane's visible tab is the
    /// graph pane. What the scene projects, and what the canvas prepass gates
    /// on.
    pub(crate) fn shows_graph(&self) -> bool {
        self.layout
            .groups()
            .any(|group| matches!(group.active_tab(), TabRef::Graph))
    }

    /// [`node_alive`] asked of the whole document — the form every
    /// `NodeId`-keyed cache's sweep takes. See there for why this one question
    /// answers for all of them.
    pub(crate) fn holds_node(&self, node_id: NodeId) -> bool {
        node_alive(&self.graph, node_id)
    }

    /// [`node_alive`] narrowed to preview nodes — what retains the value one
    /// published.
    ///
    /// Deliberately stricter than the shared rule rather than accidentally
    /// different, which is why it is spelled as a narrowing: the preview store
    /// holds textures up to 8192² RGBA8, so a value whose key is not a live
    /// preview node releases at the next sweep instead of riding on the
    /// node's own lifetime. One node backs one on-screen card, so the node's
    /// own id answers for the card without anything else alongside it.
    pub(crate) fn holds_preview_node(&self, node_id: NodeId) -> bool {
        self.graph.find(node_id).is_some_and(is_preview_node)
    }

    /// Every open viewer tab's preview node, visible or not — the retention
    /// half: a hidden tab still expects its value to be there when shown.
    pub(crate) fn viewer_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.layout.all_tabs().filter_map(|tab| match tab {
            TabRef::ImageViewer(node_id) => Some(node_id),
            _ => None,
        })
    }

    /// The viewer nodes a record pass will actually draw: each group renders
    /// its *visible* tab and nothing else. Scopes full-resolution texture
    /// uploads to what's on screen — a viewer stacked behind another tab in
    /// the same pane costs nothing until it's activated.
    pub(crate) fn visible_viewer_nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.layout
            .groups()
            .filter_map(|group| match group.active_tab() {
                TabRef::ImageViewer(node_id) => Some(node_id),
                _ => None,
            })
    }

    /// Bring the editor's derived state back in line with the graph: drop
    /// tabs whose target vanished, collapsing panes that empty. The graph pane
    /// always survives — the graph and `main_view` both always exist — so this
    /// only ever prunes viewer tabs whose node is gone.
    ///
    /// Runs every frame in the navigation phase, right after undo/redo and the
    /// intent drain, so a stale tab can never reach a save.
    pub(crate) fn reconcile_with_graph(&mut self) {
        // Common case: every tab still resolves — touch nothing (no
        // per-frame allocation). Only when something died does the
        // retain (and its re-pack) run, against the same predicate.
        if self.layout.all_tabs().any(|t| !tab_alive(&self.graph, t)) {
            // Split the borrow so the layout retain can read `graph`.
            let Document { graph, layout, .. } = self;
            layout.retain_tabs(|t| tab_alive(graph, t));
        }
    }
}

impl From<CoreGraph> for Document {
    fn from(graph: CoreGraph) -> Self {
        let main_view = GraphView::for_graph(&graph);
        Self {
            graph,
            main_view,
            layout: DockLayout::default(),
        }
    }
}

#[cfg(test)]
pub(crate) mod harness;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::document::dock::DockOp;
    use crate::core::document::harness::DocFixture;

    /// Every `NodeId`-keyed cache sweeps against one rule, and the two
    /// narrowings of it are strict subsets — so a node that is gone is gone by
    /// all of them at once, and nothing can be retained by one sweep while
    /// another has released it.
    ///
    /// Re-diverging the predicates is what this catches: they were four
    /// separate spellings of `graph.find(..).is_some()` before, and nothing
    /// said which were meant to differ.
    #[test]
    fn node_liveness_is_one_rule_with_two_declared_narrowings() {
        let mut fixture = DocFixture::default();
        let preview_func = crate::core::preview::preview_func(Default::default());
        fixture.library.add(preview_func.clone());
        let preview = fixture.doc.graph.add(Node::from(&preview_func));
        let plain = fixture.stub_at(Vec2::ZERO);
        let mut doc = fixture.doc;

        // The shared rule answers for both kinds; the preview narrowing is a
        // strict subset of it, never a different question.
        for node in [preview, plain] {
            assert!(doc.holds_node(node), "the document holds {node:?}");
            assert!(
                !doc.holds_preview_node(node) || doc.holds_node(node),
                "the preview narrowing cannot outlive the rule it narrows"
            );
        }
        assert!(doc.holds_preview_node(preview));
        assert!(
            !doc.holds_preview_node(plain),
            "a stub node retains no published value"
        );

        // The tab lift is the same rule asked of a tab: a viewer dies exactly
        // with its node, and the two non-node tabs never do.
        assert!(tab_alive(&doc.graph, TabRef::ImageViewer(preview)));
        assert!(tab_alive(&doc.graph, TabRef::Graph));
        assert!(tab_alive(&doc.graph, TabRef::Preferences));

        doc.graph.detach_node(preview);
        assert!(!doc.holds_node(preview), "the rule says gone");
        assert!(
            !doc.holds_preview_node(preview),
            "and so does the narrowing"
        );
        assert!(
            !tab_alive(&doc.graph, TabRef::ImageViewer(preview)),
            "and so does the tab lift"
        );
        assert!(doc.holds_node(plain), "the surviving node is untouched");
    }

    /// A viewer tab retains its node's value while open, and only the pane's
    /// *visible* tab is owed a full-resolution texture.
    #[test]
    fn viewer_tabs_retain_their_node_and_only_the_visible_one_draws() {
        let mut fixture = DocFixture::default();
        let root_node = fixture.stub_at(Vec2::ZERO);
        let doc = &mut fixture.doc;
        assert_eq!(doc.viewer_nodes().count(), 0);

        let primary = doc.layout.primary().id;
        doc.layout
            .find_or_insert(TabRef::ImageViewer(root_node), primary);
        assert_eq!(
            doc.viewer_nodes().collect::<Vec<_>>(),
            vec![root_node],
            "an open viewer tab retains exactly its own node"
        );

        // Retention and visibility are different questions: the tab is open
        // (so its value must be kept) but the pane still shows the graph tab,
        // so nothing draws it and no full-resolution texture is owed.
        assert_eq!(doc.visible_viewer_nodes().count(), 0);
        doc.layout.apply(DockOp::ActivateTab {
            tab: TabRef::ImageViewer(root_node),
        });
        assert_eq!(
            doc.visible_viewer_nodes().collect::<Vec<_>>(),
            vec![root_node],
            "activating the tab makes it the pane's drawn viewer"
        );

        doc.layout.apply(DockOp::CloseTab {
            tab: TabRef::ImageViewer(root_node),
        });
        assert_eq!(
            doc.viewer_nodes().count(),
            0,
            "closing the viewer leaves nothing retained"
        );
    }

    #[test]
    fn dock_layout_round_trips_as_json() {
        use crate::core::document::dock::{DockDrop, SplitSide};

        let mut doc = DocFixture::sample().doc;
        let node_id = doc.graph.iter().next().unwrap().id;
        let primary = doc.layout.primary().id;
        doc.layout.find_or_insert(TabRef::Preferences, primary);
        doc.layout
            .find_or_insert(TabRef::ImageViewer(node_id), primary);
        // A split pane too, so the whole tree shape round-trips — not
        // just a flat strip.
        doc.layout.apply(DockOp::MoveTab {
            tab: TabRef::ImageViewer(node_id),
            to: DockDrop::Split {
                group: primary,
                side: SplitSide::Right,
            },
        });
        let bytes = serde_json::to_vec_pretty(&doc).expect("serialize with dock layout");
        let back: Document = serde_json::from_slice(&bytes).expect("deserialize");
        back.validate().expect("round-tripped document is valid");
        assert_eq!(
            back.layout, doc.layout,
            "the split tree (groups, focus, ratio) round-trips through JSON"
        );
    }

    #[test]
    fn document_passes_validation() {
        DocFixture::sample().doc.validate().unwrap();
    }

    #[test]
    #[should_panic(expected = "view item to move must exist")]
    fn moving_missing_view_item_panics() {
        GraphView::default().move_item_to_index(&NodeId::unique(), 0);
    }
}
