pub(crate) mod dock;
pub(crate) mod open_document;
pub(crate) mod validate;

use ::serde::{Deserialize, Serialize};
use glam::Vec2;
use scenarium::{DetachedNode, Graph as CoreGraph, InputPort, NodeId, NodeKind, OutputPort};
use std::collections::{BTreeMap, BTreeSet};

use crate::core::document::dock::DockLayout;
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

/// An affine 2D camera: `local = pan + zoom * content`. One value shared by
/// the persisted per-graph [`GraphView`], the `GraphCtx` a canvas reads it
/// through, and the `SetViewport` edit, so the three can't drift on field
/// names or semantics.
///
/// The graph canvas reads `pan` as a canvas-local px offset and `zoom` as a
/// world scale factor. The image viewer reuses the same algebra over a
/// different content space — `pan` is the image's top-left offset in
/// pane-local px, `zoom` is display px per texel — which is why
/// `pan_zoom::zoom_about` and `fold_scroll_zoom` serve both.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Viewport {
    pub(crate) pan: Vec2,
    pub(crate) zoom: f32,
}

impl Viewport {
    /// Invert the camera: a point in the surface's local space → the point of
    /// *content* under it. The exact inverse of the `local = pan + zoom *
    /// content` this type documents, so it is defined here rather than
    /// re-derived by each reader.
    ///
    /// The graph canvas calls its content space "world" — node positions, wire
    /// endpoints and the cull region all live in it — and is the only caller
    /// today; the image viewer needs the forward direction instead.
    pub(crate) fn to_world(self, local: Vec2) -> Vec2 {
        (local - self.pan) / self.zoom
    }

    /// Whether this is a *persistable graph* camera — what
    /// `GraphViewValidationError::InvalidViewport` reports on. Only the
    /// document's own viewport round-trips through serde and so needs
    /// checking on the way back in; the image viewer's is rebuilt from the
    /// pane every session and never validated.
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

/// Where one node body sits and how far forward it paints.
///
/// The two are independent: dragging a node changes `pos` and nothing else,
/// raising it changes `z` and nothing else. Keeping the stacking order as a
/// *value* here rather than as the position of an entry in an ordered map is
/// what lets [`GraphView`] derive its equality and serde, and what makes
/// `Raise` a field write instead of a reorder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ItemPlacement {
    /// Canvas-world position of the body's top-left corner.
    pub(crate) pos: Vec2,
    /// Paint depth: higher paints later, so it sits in front. Ties break by
    /// `NodeId`, so the order is total and stable across frames — see
    /// [`GraphView::paint_order`].
    pub(crate) z: u32,
}

/// Editor-side view metadata for the document's graph: per-item placements,
/// the viewport, and the selection (`Document::main_view`). The graph *data*
/// itself lives in the core `Graph`; this is purely how the editor presents
/// and navigates it.
///
/// **Everything here is persisted and undoable, by design** — reopening
/// a file restores the exact camera and selection, and Ctrl+Z walks
/// camera/selection changes alongside structural edits (see the long
/// note that used to live on `Document`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GraphView {
    /// Every node body's placement. Exactly one entry per graph node —
    /// enforced by [`Self::validate`].
    ///
    /// Order-insensitive on purpose: paint order rides in each entry's
    /// [`ItemPlacement::z`], so this map is free to be keyed and compared as a
    /// set. Ten of the eleven callers that walk the nodes don't care about
    /// stacking at all, and the one that does asks for [`Self::paint_order`].
    pub(crate) item_placements: BTreeMap<NodeId, ItemPlacement>,
    pub(crate) viewport: Viewport,
    /// `BTreeSet` so equality and serialization are order-independent
    /// (no spurious undo entries from reordering).
    pub(crate) selected: BTreeSet<NodeId>,
}

impl GraphView {
    /// A fresh view over `graph`: one zero-positioned, unraised placement per
    /// node, the default camera, nothing selected.
    ///
    /// The only way a view is built from a graph, so there is nowhere for the
    /// two to disagree about which nodes are placed — which is exactly what
    /// [`Self::validate`] checks on the way back in from disk.
    pub(crate) fn new(graph: &CoreGraph) -> Self {
        Self {
            item_placements: graph
                .iter()
                .map(|node| (node.id, ItemPlacement::default()))
                .collect(),
            ..Self::default()
        }
    }

    /// The items back-to-front: what a paint pass draws in order, and the only
    /// place stacking is resolved.
    ///
    /// `(z, NodeId)` rather than `z` alone so the sort is total — several
    /// items share the seed `z` of 0 until something raises them, and a
    /// comparator that let those tie would leave their relative order at the
    /// mercy of the sort's stability.
    pub(crate) fn paint_order(&self) -> Vec<(NodeId, ItemPlacement)> {
        let mut items: Vec<(NodeId, ItemPlacement)> =
            self.item_placements.iter().map(|(k, v)| (*k, *v)).collect();
        items.sort_unstable_by_key(|(id, placement)| (placement.z, *id));
        items
    }

    /// The depth that puts an item in front of every other. One past the
    /// current maximum, so a raise never has to renumber its neighbours.
    pub(crate) fn front_z(&self) -> u32 {
        self.item_placements
            .values()
            .map(|placement| placement.z)
            .max()
            .map_or(0, |top| top.saturating_add(1))
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

impl Document {
    /// Drop a node from both the graph and the view (its placement and any
    /// selection membership) — the one edit that has to touch both to leave
    /// the document consistent.
    pub(crate) fn remove_node(&mut self, node_id: NodeId) -> DetachedNode {
        self.main_view.item_placements.remove(&node_id);
        let detached = self.graph.detach_node(node_id);
        self.main_view.selected.remove(&node_id);
        detached
    }

    /// Whether the graph is on a canvas — i.e. some pane's visible tab is the
    /// graph pane. What the scene projects, and what the canvas prepass gates
    /// on.
    pub(crate) fn shows_graph(&self) -> bool {
        self.layout.active_tabs().any(|tab| tab == TabRef::Graph)
    }

    /// Whether the graph still holds `node_id`.
    ///
    /// **The** liveness question, and the single definition of it. Node ids
    /// are never reused, so a node absent here is gone for good — which is
    /// what makes this the retention rule for every `NodeId`-keyed cache that
    /// outlives the scene: the canvas geometry, the preview values, the open
    /// inspectors. Absence from a *scene* means only that a node is off-screen
    /// or on a closed tab; absence from here is permanent.
    ///
    /// **The caches that ask it**, and where each is swept:
    ///
    /// | Cache | Swept by |
    /// |---|---|
    /// | `CanvasGeometry`'s port offsets + node sizes | `GraphUI::retain_nodes` |
    /// | `Inspectors::modes` | `GraphUI::retain_nodes` |
    /// | `PreviewStore::entries` (through [`Self::holds_preview_node`]) | `PreviewStore::reconcile` |
    ///
    /// All three run from `App::update`, once a frame,
    /// alongside `MainWindow::image_viewers` — which is swept in the same pass
    /// but against the *layout*, since a viewer's framing dies with its tab
    /// rather than with its node. A new cache derived from the document
    /// belongs in that pass and on this list; nothing else enforces it, so
    /// this is where to look and what to extend.
    ///
    /// Two neighbours are deliberately outside it: [`Self::reconcile_with_graph`]
    /// is the document repairing *itself* rather than a cache being swept, and
    /// must stay in the navigation phase so a stale tab cannot reach the record
    /// or a save; and `RunState::nodes` is a record of the last run rather than
    /// a cache derived from the document — see its field doc for the reasoning.
    pub(crate) fn holds_node(&self, node_id: NodeId) -> bool {
        self.graph.find(node_id).is_some()
    }

    /// [`Self::holds_node`] asked of a tab: the graph pane and `Preferences`
    /// always resolve, and a viewer tab dies exactly with its node. The single
    /// predicate behind [`Self::reconcile_with_graph`]'s fast path, its prune,
    /// and [`Self::validate`]'s tab check, so the three can't drift.
    pub(crate) fn holds_tab(&self, tab: TabRef) -> bool {
        match tab {
            TabRef::Graph | TabRef::Preferences => true,
            TabRef::ImageViewer(node_id) => self.holds_node(node_id),
        }
    }

    /// [`Self::holds_node`] narrowed to preview nodes — what retains the value one
    /// published.
    ///
    /// Deliberately stricter than the shared rule rather than accidentally
    /// different, which is why it is spelled as a narrowing: the preview store
    /// holds textures up to 8192² RGBA8, so a value whose key is not a live
    /// preview node releases at the next sweep instead of riding on the
    /// node's own lifetime. One node backs one on-screen card, so the node's
    /// own id answers for the card without anything else alongside it.
    pub(crate) fn holds_preview_node(&self, node_id: NodeId) -> bool {
        self.graph
            .find(node_id)
            .is_some_and(|node| match node.kind {
                NodeKind::Func(func_id) => preview::is_preview(func_id),
                _ => false,
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
        // Common case: every tab still resolves, so touch nothing.
        // `retain_tabs` rebuilds the whole node tree through `normalize`,
        // allocating as it goes, and must not run on a frame with nothing to
        // prune.
        if self.layout.all_tabs().all(|tab| self.holds_tab(tab)) {
            return;
        }
        // The retain wants `layout` mutably while the predicate reads the rest
        // of the document, which one `&mut self` cannot hand out. Moving the
        // layout out for the call splits them: the predicate borrows a
        // `Document` whose layout is a placeholder, and it never asks about
        // one. Cold path only — the fast path above already returned.
        let mut layout = std::mem::take(&mut self.layout);
        layout.retain_tabs(|tab| self.holds_tab(tab));
        self.layout = layout;
    }
}

impl From<CoreGraph> for Document {
    fn from(graph: CoreGraph) -> Self {
        let main_view = GraphView::new(&graph);
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
    use scenarium::Node;

    use super::*;
    use crate::core::document::dock::dock_op::DockOp;
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
        assert!(doc.holds_tab(TabRef::ImageViewer(preview)));
        assert!(doc.holds_tab(TabRef::Graph));
        assert!(doc.holds_tab(TabRef::Preferences));

        doc.graph.detach_node(preview);
        assert!(!doc.holds_node(preview), "the rule says gone");
        assert!(
            !doc.holds_preview_node(preview),
            "and so does the narrowing"
        );
        assert!(
            !doc.holds_tab(TabRef::ImageViewer(preview)),
            "and so does the tab lift"
        );
        assert!(doc.holds_node(plain), "the surviving node is untouched");
    }

    #[test]
    fn dock_layout_round_trips_as_json() {
        use crate::core::document::dock::dock_op::DockDrop;
        use crate::core::document::dock::split_side::SplitSide;

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

    /// Paint order is a total order over `(z, NodeId)`, so items sharing the
    /// seed depth still come out in a fixed sequence, and `front_z` puts a
    /// raise past every one of them. Both matter for stability: equal depths
    /// are the common case until something is raised.
    #[test]
    fn paint_order_is_total_and_front_z_clears_every_item() {
        let mut view = GraphView::default();
        assert_eq!(view.front_z(), 0, "an empty view seeds at zero");

        let (low, high) = (NodeId::from_u128(1), NodeId::from_u128(2));
        view.item_placements.insert(high, ItemPlacement::default());
        view.item_placements.insert(low, ItemPlacement::default());
        assert_eq!(
            view.paint_order()
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            [low, high],
            "equal depths break by id, not by insertion"
        );

        assert_eq!(view.front_z(), 1, "one past the shared depth of zero");
        view.item_placements.get_mut(&low).unwrap().z = view.front_z();
        assert_eq!(
            view.paint_order()
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            [high, low],
            "raising the lower id lifts it past the higher one"
        );
        assert_eq!(view.front_z(), 2, "and the next raise clears that one too");
    }
}
