pub(crate) mod auto_layout;
pub(crate) mod dock;
mod serde;
pub(crate) mod validate;

use ::serde::{Deserialize, Serialize};
use glam::Vec2;
use indexmap::IndexMap;
use scenarium::GraphId;
use scenarium::GraphLink;
use scenarium::{DetachedNode, Graph as CoreGraph, NodeId, NodeSearch, OutputPort};
use scenarium::{Node, NodeKind};
use std::collections::{BTreeSet, HashMap};

use crate::core::document::auto_layout::AUTO_LAYOUT_ORIGIN;
use crate::core::document::dock::DockLayout;

/// Initial placement of a fresh graph's boundary nodes: the input
/// boundary at the origin, the output boundary one gap to the right and
/// level with it (instead of the generic auto-layout stacking the two
/// unconnected nodes in one column).
const BOUNDARY_LAYOUT_GAP: f32 = 520.0;

/// Which graph an editor tab is pointed at. `Main` is the document root;
/// `Local(id)` addresses a nested graph in `Document::graph.graphs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum GraphRef {
    Main,
    Local(GraphId),
}

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
/// its `WidgetId` (see `crate::gui::node::port_row::port_circle_wid`)
/// without threading a cache, and serializable so a persisted tab
/// ([`TabRef::ImageViewer`]) can bind to it. Node ids are unique across
/// the whole document (graph interiors included), so no graph ref is
/// needed alongside — upheld by `Graph::fresh_copy` at every copy
/// boundary (import/localize/detach) and enforced by
/// [`Document::validate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct PortRef {
    pub node_id: NodeId,
    pub kind: PortKind,
    pub port_idx: usize,
}

/// What an editor tab shows. Most tabs are graphs — the root and any
/// opened graph interiors ([`TabRef::Graph`]) — but a tab can also be
/// a non-graph app view like [`TabRef::Preferences`] (the settings window).
/// Persisted + undoable like the rest of the tab/view state, so reopening
/// a document restores its open tabs and Ctrl+Z walks tab open/close.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TabRef {
    /// A graph pane (root or a local graph interior).
    Graph(GraphRef),
    /// The app-preferences / settings view — no graph, no canvas.
    Preferences,
    /// A full-resolution viewer of one port's runtime image — one tab per
    /// port, deduped on open. Content is runtime-only
    /// (`crate::gui::image_viewer`): a restored tab pulls any current value
    /// from `RunState` when drawn. Pruned when its node is deleted, like a
    /// graph tab whose def vanished.
    ImageViewer(PortRef),
}

/// Which side of a graph interface a boundary-port edit targets.
///
/// The side names the *interface*, not the UI column it shows in: a
/// boundary node mirrors the interface, so the `GraphOutput` node's
/// *input column* edits the `Output` side and the `GraphInput` node's
/// *output column* edits the `Input` side (see `gui::node::port_row`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum BoundarySide {
    Input,
    Output,
}

/// One canvas item's identity: a node body or a pinned output's floating
/// preview widget — the [`GraphRef`]/[`PortRef`]/[`TabRef`] sibling for
/// the things that occupy canvas space. The two kinds share every
/// item-level mechanism: one selection set (`GraphView::selected` stays a
/// single `BTreeSet` — click to select, Shift-click to toggle,
/// rubber-band sweeps both kinds in), one paint stack
/// (`GraphView::item_placements`, keyed by this — `Intent::Raise` lifts either
/// kind), and one group-drag path (`Intent::MoveSelection`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) enum ItemRef {
    Node(NodeId),
    Pin(OutputPort),
}

impl ItemRef {
    /// Whether this key names something that lives on `node_id` — the node
    /// itself, or one of its pinned outputs. Used to prune a node's
    /// selection membership and view items (both forms) when it's removed
    /// from the graph.
    pub(crate) fn belongs_to(self, node_id: NodeId) -> bool {
        match self {
            ItemRef::Node(id) => id == node_id,
            ItemRef::Pin(port) => port.node_id == node_id,
        }
    }
}

/// A graph's camera: pan offset (canvas-local px) + zoom factor. One
/// value shared by the persisted per-graph [`GraphView`], the per-frame
/// `Scene` projection, and the `SetViewport` edit, so the three can't
/// drift on field names or semantics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Viewport {
    pub pan: Vec2,
    pub zoom: f32,
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

/// Editor-side view metadata for one graph: per-item positions and paint
/// order, the viewport, and the selection. One of these exists per
/// open/edited graph (the root in `Document::main_view`, each graph
/// interior in `Document::local_views`). The graph *data* itself lives in
/// the core `Graph`; this is purely how the editor presents and
/// navigates it.
///
/// **Everything here is persisted and undoable, by design** — reopening
/// a file restores the exact camera and selection, and Ctrl+Z walks
/// camera/selection changes alongside structural edits (see the long
/// note that used to live on `Document`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GraphView {
    /// Node bodies and pinned-output preview widgets in one list — the
    /// canvas's **paint stack**: the order is the shared z-order (later
    /// items draw in front), so a preview can sit above or below any
    /// node independently, and `Intent::Raise` lifts either kind to the
    /// top. Exactly one `Node` item per graph node, exactly one `Pin`
    /// item per *currently pinned* output (unpinning removes the item;
    /// undo restores it, slot included) — enforced by [`Self::validate`].
    ///
    /// A pin's position is absolute rather than port-relative: the
    /// widget sits where it was put and does *not* follow its node when
    /// that node is moved — only its wire re-routes, like a connection
    /// to another node would.
    #[serde(with = "crate::core::document::serde")]
    pub item_placements: IndexMap<ItemRef, Vec2>,
    pub viewport: Viewport,
    /// `BTreeSet` so equality and serialization are order-independent
    /// (no spurious undo entries from reordering). Holds both node bodies
    /// and pinned-output preview widgets — see [`ItemRef`].
    pub selected: BTreeSet<ItemRef>,
}

impl PartialEq for GraphView {
    fn eq(&self, other: &Self) -> bool {
        self.viewport == other.viewport
            && self.selected == other.selected
            && self.item_placements.len() == other.item_placements.len()
            && self
                .item_placements
                .iter()
                .zip(&other.item_placements)
                .all(|(left, right)| left == right)
    }
}

impl Eq for GraphView {}

impl GraphView {
    /// A fresh view seeded with a zero-positioned item for every node in
    /// `graph` and every pinned output (callers usually `auto_layout`
    /// right after, which places both kinds).
    pub(crate) fn for_graph(graph: &CoreGraph) -> Self {
        let mut item_placements = IndexMap::with_capacity(graph.len());
        for node in graph.iter() {
            item_placements.insert(ItemRef::Node(node.id), Vec2::ZERO);
        }
        for port in graph.pinned_outputs() {
            item_placements.insert(ItemRef::Pin(port), Vec2::ZERO);
        }
        Self {
            item_placements,
            ..Default::default()
        }
    }

    pub(crate) fn move_item_to_index(&mut self, key: &ItemRef, target_index: usize) {
        let from = self
            .item_placements
            .get_index_of(key)
            .expect("view item to move must exist");
        let to = target_index.min(self.item_placements.len() - 1);
        self.item_placements.move_index(from, to);
    }
}

/// A graph plus its view metadata, borrowed together so an edit can
/// touch both atomically without re-borrowing `Document` through two
/// methods (the borrow checker can't prove `graph` and `view` are
/// disjoint across separate accessor calls). Mutable counterpart of
/// [`EditScopeRef`].
pub(crate) struct EditScope<'a> {
    pub graph: &'a mut CoreGraph,
    pub view: &'a mut GraphView,
}

/// Read-only graph + view pair, for `build_step`'s pre-mutation reads.
pub(crate) struct EditScopeRef<'a> {
    pub graph: &'a CoreGraph,
    pub view: &'a GraphView,
}

impl EditScope<'_> {
    /// Drop a node from both the graph and its view (its own item, its
    /// pinned outputs' items, and any selection membership). Mirrors the
    /// old `Document::remove_node`.
    pub(crate) fn remove_node(&mut self, node_id: &NodeId) -> DetachedNode {
        self.view
            .item_placements
            .retain(|key, _| !key.belongs_to(*node_id));
        let detached = self.graph.detach_node(*node_id);
        self.view.selected.retain(|k| !k.belongs_to(*node_id));
        detached
    }
}

/// The thing being edited: the root `Graph` plus the editor view
/// metadata for each graph the user has open — the root in `main_view`,
/// every opened graph interior in `local_views`. The `Library` it
/// resolves against lives one level up on `App` (runtime-owned).
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Document {
    pub graph: CoreGraph,
    pub main_view: GraphView,
    /// View metadata for local graph interiors, created lazily when a
    /// graph is first opened in a tab. Keyed by `GraphId`.
    #[serde(default)]
    pub local_views: HashMap<GraphId, GraphView>,
    /// The pane arrangement: open tabs grouped into split panes, plus
    /// the focused group. Persisted + undoable like the rest of the view
    /// state (every layout mutation is an undoable `Intent::Dock`).
    #[serde(default)]
    pub layout: DockLayout,
}

/// Whether a tab still resolves against the graph: `Main` and
/// `Preferences` always do, a local graph tab lives with its map entry,
/// and a viewer tab dies with its node. The single predicate behind
/// [`Document::ensure_valid_layout`]'s fast-path *and* its prune, so
/// the two can't drift.
fn tab_alive(graph: &CoreGraph, tab: TabRef) -> bool {
    match tab {
        TabRef::Graph(GraphRef::Main) | TabRef::Preferences => true,
        TabRef::Graph(GraphRef::Local(id)) => graph.graphs.contains_key(&id),
        TabRef::ImageViewer(port) => graph.find(&port.node_id, NodeSearch::Recursive).is_some(),
    }
}

/// Index of the slot named `expected`: `idx_hint` when it still holds
/// that name (fast path / duplicate-name disambiguation), else the first
/// slot matching by name. `None` when nothing matches.
fn resolve_named_slot<T>(
    slots: &[T],
    idx_hint: usize,
    expected: &str,
    name_of: impl Fn(&T) -> &str,
) -> Option<usize> {
    if let Some(s) = slots.get(idx_hint)
        && name_of(s) == expected
    {
        return Some(idx_hint);
    }
    slots.iter().position(|s| name_of(s) == expected)
}

impl Document {
    /// The graph a target points at, or `None` if it no longer exists
    /// (e.g. a graph deleted while its tab was open).
    pub(crate) fn graph_for(&self, target: GraphRef) -> Option<&CoreGraph> {
        match target {
            GraphRef::Main => Some(&self.graph),
            GraphRef::Local(id) => self.graph.graphs.get(&id),
        }
    }

    /// The view metadata for a target, or `None` when unopened/missing.
    pub(crate) fn view(&self, target: GraphRef) -> Option<&GraphView> {
        match target {
            GraphRef::Main => Some(&self.main_view),
            GraphRef::Local(id) => self.local_views.get(&id),
        }
    }

    pub(crate) fn graph_mut(&mut self, target: GraphRef) -> Option<&mut CoreGraph> {
        match target {
            GraphRef::Main => Some(&mut self.graph),
            GraphRef::Local(id) => self.graph.graphs.get_mut(&id),
        }
    }

    /// Graph + view borrowed together for editing the given target.
    pub(crate) fn scope_mut(&mut self, target: GraphRef) -> Option<EditScope<'_>> {
        match target {
            GraphRef::Main => Some(EditScope {
                graph: &mut self.graph,
                view: &mut self.main_view,
            }),
            GraphRef::Local(id) => {
                let graph = self.graph.graphs.get_mut(&id)?;
                let view = self.local_views.get_mut(&id)?;
                Some(EditScope { graph, view })
            }
        }
    }

    /// Read-only graph + view pair for the given target.
    pub(crate) fn scope(&self, target: GraphRef) -> Option<EditScopeRef<'_>> {
        Some(EditScopeRef {
            graph: self.graph_for(target)?,
            view: self.view(target)?,
        })
    }

    /// The graph on the canvas: the *primary* group's visible tab, when
    /// it's a graph — `None` when that pane shows a non-graph view.
    /// Independent of `layout.focused`: only the primary group hosts
    /// canvases, and its graph stays visible (and editable) while focus
    /// sits in another pane.
    pub(crate) fn active_target(&self) -> Option<GraphRef> {
        match self.layout.primary().active_tab() {
            TabRef::Graph(target) => Some(target),
            TabRef::Preferences | TabRef::ImageViewer(_) => None,
        }
    }

    pub(crate) fn is_output_pinned(&self, port: OutputPort) -> bool {
        fn graph_contains(graph: &CoreGraph, port: OutputPort) -> bool {
            graph.is_output_pinned(port)
                || graph
                    .graphs
                    .values()
                    .any(|nested| graph_contains(nested, port))
        }

        graph_contains(&self.graph, port)
    }

    pub(crate) fn viewer_outputs(&self) -> impl Iterator<Item = OutputPort> + '_ {
        self.layout.all_tabs().filter_map(|tab| match tab {
            TabRef::ImageViewer(PortRef {
                node_id,
                kind: PortKind::Output,
                port_idx,
            }) => Some(OutputPort::new(node_id, port_idx)),
            _ => None,
        })
    }

    pub(crate) fn retains_output_resource(&self, port: OutputPort) -> bool {
        self.is_output_pinned(port) || self.viewer_outputs().any(|viewer_port| viewer_port == port)
    }

    /// Ensure a `GraphView` exists for a local graph interior,
    /// auto-laying-out its nodes on first creation. Returns `false` if
    /// the graph no longer exists.
    pub(crate) fn ensure_sub_view(&mut self, id: GraphId) -> bool {
        if self.local_views.contains_key(&id) {
            return true;
        }
        let view = {
            let Some(graph) = self.graph.graphs.get(&id) else {
                return false;
            };
            let mut view = GraphView::for_graph(graph);
            view.auto_layout(graph);
            view
        };
        self.local_views.insert(id, view);
        true
    }

    /// Keep the layout renderable: drop tabs whose graph vanished
    /// (collapsing panes that empty) and seed any `Local` tab that's
    /// missing its view metadata — so the scene rebuild always resolves
    /// a live graph *and* view. `Main` always survives (`graph_for(Main)`
    /// is infallible and `main_view` always exists).
    ///
    /// The view-seeding covers a desync hazard: the layout and
    /// `local_views` are independent serialized fields, so a deserialized
    /// (or hand-edited) document can carry a `Local` tab with no matching
    /// `local_views` entry. Seeding it here recovers gracefully instead of
    /// panicking on a later `view(target).expect(..)`.
    pub(crate) fn ensure_valid_layout(&mut self) {
        // Common case: every tab still resolves — touch nothing (no
        // per-frame allocation). Only when something died does the
        // retain (and its re-pack) run, against the same predicate.
        if self.layout.all_tabs().any(|t| !tab_alive(&self.graph, t)) {
            // Split the borrow so the layout retain can read `graph`.
            let Document { graph, layout, .. } = self;
            layout.retain_tabs(|t| tab_alive(graph, t));
        }
        // Seed views for any `Local` tab missing one. Guarded by `any`
        // so the common (all-seeded) case allocates nothing.
        if self.layout.all_tabs().any(
            |t| matches!(t, TabRef::Graph(GraphRef::Local(id)) if !self.local_views.contains_key(&id)),
        ) {
            let missing: Vec<GraphId> = self
                .layout
                .all_tabs()
                .filter_map(|t| match t {
                    TabRef::Graph(GraphRef::Local(id)) if !self.local_views.contains_key(&id) => {
                        Some(id)
                    }
                    _ => None,
                })
                .collect();
            for id in missing {
                self.ensure_sub_view(id);
            }
        }
    }

    /// Add an imported graph to this document's local graph map.
    /// [`Graph::fresh_copy`] gives every copied node fresh identity so
    /// repeated imports preserve document-wide node-id uniqueness.
    /// `origin` is carried over so a re-imported asset keeps its library
    /// lineage for Publish. The undo stack is unaffected: no existing
    /// history references the freshly added graph.
    pub(crate) fn import_graph(&mut self, graph: CoreGraph) -> GraphId {
        let id = GraphId::unique();
        let origin = graph.origin;
        let mut copy = graph.fresh_copy();
        copy.origin = origin;
        self.graph.insert_graph(id, copy);
        id
    }

    /// Create a fresh, empty local graph with its two boundary nodes.
    pub(crate) fn create_graph(&mut self) -> GraphId {
        let id = GraphId::unique();
        let mut graph = CoreGraph::new(format!("graph {}", self.graph.graphs.len() + 1));
        let input = Node::new(NodeKind::GraphInput);
        let output = Node::new(NodeKind::GraphOutput);
        let input_id = graph.add(input);
        let output_id = graph.add(output);
        let inst = Node::graph_instance(&graph, GraphLink::Local(id));
        let inst_pos = Vec2::new(60.0, 60.0) + Vec2::splat(36.0) * self.graph.graphs.len() as f32;
        self.graph.insert_graph(id, graph);
        let inst_id = self.graph.add(inst);
        self.main_view
            .item_placements
            .insert(ItemRef::Node(inst_id), inst_pos);

        let mut view = GraphView::default();
        view.item_placements
            .insert(ItemRef::Node(input_id), AUTO_LAYOUT_ORIGIN);
        view.item_placements.insert(
            ItemRef::Node(output_id),
            AUTO_LAYOUT_ORIGIN + Vec2::new(BOUNDARY_LAYOUT_GAP, 0.0),
        );
        self.local_views.insert(id, view);

        id
    }

    /// Current name of a nested graph interface port.
    pub(crate) fn boundary_port_name(
        &self,
        graph_id: GraphId,
        side: BoundarySide,
        idx: usize,
    ) -> Option<&str> {
        let graph = self.graph.graphs.get(&graph_id)?;
        let name = match side {
            BoundarySide::Input => &graph.inputs.get(idx)?.name,
            BoundarySide::Output => &graph.outputs.get(idx)?.name,
        };
        Some(name)
    }

    /// Rename the interface port currently named `expected` on `side` to
    /// `new`. `idx_hint` is tried first (exact when nothing moved, and it
    /// disambiguates duplicate names); otherwise the slot is found by its
    /// `expected` name. Resolving by name lets undo/redo survive
    /// normalization compacting the interface — it renumbers
    /// indices but *preserves names*, so the renamed slot is still found
    /// at its new index. No-op if no matching slot exists (e.g. the port
    /// was disconnected away entirely).
    pub(crate) fn rename_boundary_port(
        &mut self,
        graph_id: GraphId,
        side: BoundarySide,
        idx_hint: usize,
        expected: &str,
        new: &str,
    ) {
        let Some(graph) = self.graph.graphs.get_mut(&graph_id) else {
            return;
        };
        let slot = match side {
            BoundarySide::Input => {
                resolve_named_slot(&graph.inputs, idx_hint, expected, |i| &i.name)
                    .map(|i| &mut graph.inputs[i].name)
            }
            BoundarySide::Output => {
                resolve_named_slot(&graph.outputs, idx_hint, expected, |o| &o.name)
                    .map(|i| &mut graph.outputs[i].name)
            }
        };
        if let Some(slot) = slot {
            *slot = new.to_owned();
        }
    }
}

impl Eq for Document {}

impl From<CoreGraph> for Document {
    fn from(graph: CoreGraph) -> Self {
        let main_view = GraphView::for_graph(&graph);
        Self {
            graph,
            main_view,
            local_views: HashMap::new(),
            layout: DockLayout::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::FuncId;
    use scenarium::testing::test_graph as core_test_graph;

    fn leaf_graph(name: &str) -> CoreGraph {
        CoreGraph::new(name)
    }

    #[test]
    fn import_regenerates_ids_and_keeps_nested_def_ids() {
        // Real storage shape: a child def lives in its *parent's* interior
        // `graph.graphs`, instanced by an interior node — not in a flat
        // root table. Importing the parent carries the child with it.
        let child_id = GraphId::unique();
        let parent_id = GraphId::unique();
        let origin_id = GraphId::unique();
        let mut parent = CoreGraph::new("parent").origin(origin_id);
        parent.insert_graph(child_id, leaf_graph("child"));
        parent.add(Node::new(NodeKind::Graph(GraphLink::Local(child_id))));
        let source_ids: Vec<NodeId> = parent.iter().map(|n| n.id).collect();

        let mut doc = Document::default();
        let id_a = doc.import_graph(parent.clone());
        let id_b = doc.import_graph(parent);

        assert_ne!(id_a, parent_id, "top-level id is regenerated");
        assert_ne!(id_a, id_b, "each import is its own def");
        assert!(
            doc.graph.graphs.get(&parent_id).is_none(),
            "original top id is not reused"
        );
        let interior_ids = |id: GraphId| -> Vec<NodeId> {
            let def = doc.graph.graphs.get(&id).expect("def resolves");
            assert_eq!(
                def.origin,
                Some(origin_id),
                "library lineage is carried over"
            );
            // The nested child def rides along under its (level-scoped) id.
            assert_eq!(def.graphs.len(), 1);
            assert!(
                def.graphs.contains_key(&child_id),
                "nested child def is preserved with its original id"
            );
            def.iter().map(|n| n.id).collect()
        };
        // Interior node ids are freshly generated per import: the copies
        // share none with the source file or each other, so the
        // document-wide uniqueness gate holds after a double import.
        let (ids_a, ids_b) = (interior_ids(id_a), interior_ids(id_b));
        assert_eq!(ids_a.len(), source_ids.len());
        for id in &ids_a {
            assert!(
                !source_ids.contains(id) && !ids_b.contains(id),
                "interior ids are fresh per import"
            );
        }
        doc.validate().unwrap();
    }

    #[test]
    fn validate_rejects_duplicate_node_ids_across_graphs() {
        // The same node id planted in the root graph and a def interior —
        // unreachable through the editor (import/localize/detach sever
        // identity via `fresh_copy`), so it's corrupt input validation refuses.
        let mut doc = Document::default();
        let node_id = add_node_at(&mut doc, Vec2::ZERO);
        let graph_id = doc.create_graph();
        let dup = Node::new(NodeKind::Func(FuncId::unique()));
        doc.graph
            .graphs
            .get_mut(&graph_id)
            .unwrap()
            .insert(node_id, dup);

        let err = doc.validate().unwrap_err();
        assert!(
            format!("{err:#}").contains("occurs in more than one authoring graph"),
            "unexpected validation error: {err:#}"
        );
    }

    #[test]
    fn importing_same_def_twice_makes_two_copies() {
        let mut doc = Document::default();
        let a = doc.import_graph(leaf_graph("x"));
        let b = doc.import_graph(leaf_graph("x"));
        assert_ne!(a, b, "each import gets its own id");
        assert_eq!(doc.graph.graphs.len(), 2, "no silent overwrite");
    }

    #[test]
    fn add_node_with_def_round_trips() {
        use crate::core::edit::intent::apply::{apply_step, revert_step};
        use crate::core::edit::intent::build::build_step;
        use crate::core::edit::intent::types::Intent;

        // Instancing a library graph localizes it: a `Local` def copy
        // (recording its `origin`) is added alongside the instance node, as
        // one undoable `AddNode`.
        let lib_id = GraphId::unique();
        let mut local = leaf_graph("Lib").fresh_copy();
        local.origin = Some(lib_id);
        let local_id = GraphId::unique();
        let node = Node::graph_instance(&local, GraphLink::Local(local_id));
        let node_id = NodeId::unique();

        let mut doc = Document::default();
        let step = build_step(
            Intent::AddNode {
                pos: Vec2::ZERO,
                node_id,
                node,
                graph: Some((local_id, Box::new(local))),
                bindings: vec![],
            },
            &doc,
            GraphRef::Main,
        )
        .expect("add builds");

        apply_step(&step, &mut doc, GraphRef::Main);
        assert!(
            doc.graph.graphs.get(&local_id).is_some(),
            "local graph added alongside the instance"
        );
        assert_eq!(
            doc.graph.graphs.get(&local_id).unwrap().origin,
            Some(lib_id),
            "copy records its library origin"
        );
        assert!(
            doc.graph.find(&node_id, NodeSearch::TopLevel).is_some(),
            "instance node added"
        );

        revert_step(&step, &mut doc, GraphRef::Main);
        assert!(
            doc.graph.graphs.get(&local_id).is_none(),
            "undo removes the def"
        );
        assert!(
            doc.graph.find(&node_id, NodeSearch::TopLevel).is_none(),
            "undo removes the instance node"
        );
    }

    /// Localize one library instance into `doc`'s root graph and return
    /// `(node_id, local_def_id)`. `origin` tags the copy's library
    /// lineage so a later instance can dedup against it.
    fn add_library_instance(doc: &mut Document, lib_id: GraphId) -> (NodeId, GraphId) {
        use crate::core::edit::intent::apply::apply_step;
        use crate::core::edit::intent::build::build_step;
        use crate::core::edit::intent::types::Intent;

        let mut local = leaf_graph("Lib").fresh_copy();
        local.origin = Some(lib_id);
        let local_id = GraphId::unique();
        let node = Node::graph_instance(&local, GraphLink::Local(local_id));
        let node_id = NodeId::unique();
        let step = build_step(
            Intent::AddNode {
                pos: Vec2::ZERO,
                node_id,
                node,
                graph: Some((local_id, Box::new(local))),
                bindings: vec![],
            },
            doc,
            GraphRef::Main,
        )
        .expect("add builds");
        apply_step(&step, doc, GraphRef::Main);
        (node_id, local_id)
    }

    #[test]
    fn second_instance_reuses_existing_local_def() {
        // Two instances of the same library graph dropped into one
        // graph must share a single local graph: the first materializes the
        // localized copy, the second re-points at it (no duplicate def).
        let lib_id = GraphId::unique();
        let mut doc = Document::default();

        let (_node_a, def_a_id) = add_library_instance(&mut doc, lib_id);
        assert_eq!(doc.graph.graphs.len(), 1, "first instance adds the def");

        let (node_b, def_b_id) = add_library_instance(&mut doc, lib_id);
        assert_eq!(
            doc.graph.graphs.len(),
            1,
            "second instance reuses the def — no duplicate"
        );
        assert!(
            doc.graph.graphs.get(&def_b_id).is_none(),
            "the second fresh copy was dropped"
        );
        assert_eq!(
            doc.graph.find(&node_b, NodeSearch::TopLevel).unwrap().kind,
            NodeKind::Graph(GraphLink::Local(def_a_id)),
            "second instance points at the first instance's local graph"
        );
    }

    #[test]
    fn detach_forks_standalone_copy_and_repoints_node() {
        use crate::core::edit::intent::apply::{apply_step, revert_step};
        use crate::core::edit::intent::build::build_step;
        use crate::core::edit::intent::types::Intent;

        // A node on a library-linked local graph. Detach must fork a fresh
        // standalone copy (origin cleared), add it, and repoint the node.
        let lib_id = GraphId::unique();
        let mut doc = Document::default();
        let mut local = leaf_graph("Lib");
        local.origin = Some(lib_id);
        let local_id = GraphId::unique();
        doc.graph.insert_graph(local_id, local);
        let node = Node::graph_instance(
            doc.graph.graphs.get(&local_id).unwrap(),
            GraphLink::Local(local_id),
        );
        let node_id = doc.graph.add(node);
        doc.main_view
            .item_placements
            .insert(ItemRef::Node(node_id), Vec2::ZERO);

        let step = build_step(Intent::DetachGraph { node_id }, &doc, GraphRef::Main)
            .expect("detach builds");
        apply_step(&step, &mut doc, GraphRef::Main);

        assert_eq!(doc.graph.graphs.len(), 2, "fork adds a second local graph");
        let NodeKind::Graph(GraphLink::Local(new_id)) =
            doc.graph.find(&node_id, NodeSearch::TopLevel).unwrap().kind
        else {
            panic!("node should still be a local graph");
        };
        assert_ne!(new_id, local_id, "node now points at the fork");
        assert_eq!(
            doc.graph.graphs.get(&new_id).unwrap().origin,
            None,
            "detach clears the library lineage"
        );

        revert_step(&step, &mut doc, GraphRef::Main);
        assert_eq!(doc.graph.graphs.len(), 1, "undo drops the fork");
        let NodeKind::Graph(GraphLink::Local(restored)) =
            doc.graph.find(&node_id, NodeSearch::TopLevel).unwrap().kind
        else {
            panic!("node should still be a local graph");
        };
        assert_eq!(restored, local_id, "undo restores the original ref");
    }

    #[test]
    fn instances_of_different_library_defs_stay_separate() {
        // Different library sources must NOT collapse into one local graph.
        let mut doc = Document::default();
        add_library_instance(&mut doc, GraphId::unique());
        add_library_instance(&mut doc, GraphId::unique());
        assert_eq!(
            doc.graph.graphs.len(),
            2,
            "distinct library origins keep distinct local graphs"
        );
    }

    /// Add a bare `Func`-kind node to `doc`'s root graph + main view at
    /// `pos`, returning its id.
    fn add_node_at(doc: &mut Document, pos: Vec2) -> NodeId {
        let node = Node::new(NodeKind::Func(FuncId::unique()));
        let id = doc.graph.add(node);
        doc.main_view.item_placements.insert(ItemRef::Node(id), pos);
        id
    }

    #[test]
    fn set_disabled_round_trips_through_undo() {
        use crate::core::edit::intent::apply::{apply_step, revert_step};
        use crate::core::edit::intent::build::build_step;
        use crate::core::edit::intent::types::{Intent, NodeProperty};

        let mut doc = Document::default();
        let id = add_node_at(&mut doc, Vec2::ZERO);
        assert!(
            !doc.graph.find(&id, NodeSearch::TopLevel).unwrap().disabled,
            "starts enabled"
        );

        let step = build_step(
            Intent::SetNodeProperty {
                node_id: id,
                to: NodeProperty::Disabled(true),
            },
            &doc,
            GraphRef::Main,
        )
        .expect("builds");
        apply_step(&step, &mut doc, GraphRef::Main);
        assert!(
            doc.graph.find(&id, NodeSearch::TopLevel).unwrap().disabled,
            "apply disables"
        );

        revert_step(&step, &mut doc, GraphRef::Main);
        assert!(
            !doc.graph.find(&id, NodeSearch::TopLevel).unwrap().disabled,
            "revert re-enables (restores the captured `from`)"
        );
    }

    #[test]
    fn create_graph_has_only_boundary_nodes() {
        let mut doc = Document::default();
        let id = doc.create_graph();
        let def = doc.graph.graphs.get(&id).expect("def added");

        // Exactly the two boundary nodes, nothing else, empty interface.
        assert_eq!(def.len(), 2);
        assert_eq!(
            def.iter()
                .filter(|n| matches!(n.kind, NodeKind::GraphInput))
                .count(),
            1
        );
        assert_eq!(
            def.iter()
                .filter(|n| matches!(n.kind, NodeKind::GraphOutput))
                .count(),
            1
        );
        assert!(def.inputs.is_empty() && def.outputs.is_empty());

        // Boundary nodes are placed input-left / output-right, level.
        let input_id = def
            .iter()
            .find(|n| matches!(n.kind, NodeKind::GraphInput))
            .unwrap()
            .id;
        let output_id = def
            .iter()
            .find(|n| matches!(n.kind, NodeKind::GraphOutput))
            .unwrap()
            .id;
        let view = doc.local_views.get(&id).expect("view seeded on create");
        let ip = view
            .item_placements
            .get(&ItemRef::Node(input_id))
            .copied()
            .unwrap();
        let op = view
            .item_placements
            .get(&ItemRef::Node(output_id))
            .copied()
            .unwrap();
        assert!(op.x > ip.x, "output boundary sits right of input");
        assert_eq!(ip.y, op.y, "boundaries are level");

        // Creating also drops an instance of the new graph into root.
        let inst = doc
            .graph
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Graph(GraphLink::Local(sid)) if sid == id))
            .expect("instance added to main graph");
        assert!(
            doc.main_view
                .item_placements
                .get(&ItemRef::Node(inst.id))
                .is_some(),
            "instance has a main view item"
        );

        // Each create mints a distinct id (no overwrite).
        let id2 = doc.create_graph();
        assert_ne!(id, id2);
        assert_eq!(doc.graph.graphs.len(), 2);
    }

    /// An output-0 [`PortRef`] on `node_id`, for viewer-tab tests.
    fn out_port(node_id: NodeId) -> PortRef {
        PortRef {
            node_id,
            kind: PortKind::Output,
            port_idx: 0,
        }
    }

    /// All open tabs across the layout, for order-sensitive asserts.
    fn all_tabs(doc: &Document) -> Vec<TabRef> {
        doc.layout.all_tabs().collect()
    }

    #[test]
    fn output_resources_follow_recursive_pins_and_open_viewers() {
        let mut doc = Document::default();
        let root_node = add_node_at(&mut doc, Vec2::ZERO);
        let root_port = OutputPort::new(root_node, 0);
        assert!(!doc.retains_output_resource(root_port));

        let primary = doc.layout.primary().id;
        doc.layout
            .find_or_insert(TabRef::ImageViewer(out_port(root_node)), primary);
        assert!(doc.retains_output_resource(root_port));

        let def_id = doc.create_graph();
        let nested_node = Node::new(NodeKind::Func(FuncId::unique()));
        let definition = doc.graph.graphs.get_mut(&def_id).unwrap();
        let nested_node_id = definition.add(nested_node);
        let nested_port = OutputPort::new(nested_node_id, 0);
        definition.set_output_pinned(nested_port, true);
        assert!(
            doc.is_output_pinned(nested_port),
            "pins in nested authoring graphs retain their presentation resource"
        );
        assert!(doc.retains_output_resource(nested_port));
    }

    #[test]
    fn non_graph_tabs_have_no_target_and_survive_validation() {
        let mut doc = Document::default();
        let node_id = add_node_at(&mut doc, Vec2::ZERO);
        let primary = doc.layout.primary().id;
        // The canvas target follows the primary group's *visible* tab: a
        // graph resolves, an activated non-graph tab means no canvas.
        assert_eq!(doc.active_target(), Some(GraphRef::Main));
        doc.layout.find_or_insert(TabRef::Preferences, primary);
        doc.layout
            .find_or_insert(TabRef::ImageViewer(out_port(node_id)), primary);
        for active in [1, 2] {
            doc.layout.activate(primary, active);
            assert_eq!(doc.active_target(), None, "a non-graph tab has no target");
        }

        // Preferences always resolves; a viewer tab resolves while its
        // node exists — neither is pruned and the activation holds.
        doc.ensure_valid_layout();
        assert_eq!(
            all_tabs(&doc),
            vec![
                TabRef::Graph(GraphRef::Main),
                TabRef::Preferences,
                TabRef::ImageViewer(out_port(node_id))
            ]
        );
        assert_eq!(doc.layout.primary().active, 2);
        doc.validate_debug();
    }

    #[test]
    fn ensure_valid_layout_keeps_non_graph_tabs_when_a_graph_tab_vanishes() {
        let mut doc = Document::default();
        let node_id = add_node_at(&mut doc, Vec2::ZERO);
        let id = doc.create_graph();
        let primary = doc.layout.primary().id;
        doc.layout
            .find_or_insert(TabRef::Graph(GraphRef::Local(id)), primary);
        doc.layout.find_or_insert(TabRef::Preferences, primary);
        doc.layout
            .find_or_insert(TabRef::ImageViewer(out_port(node_id)), primary);
        doc.layout.activate(primary, 3); // viewing the image tab
        // Drop the graph out from under its open tab.
        doc.graph.graphs.remove(&id);

        doc.ensure_valid_layout();
        // The dead graph tab is pruned; Main + the non-graph tabs
        // remain, and the clamped active still points at the image tab
        // (it slid left one slot with the removal).
        assert_eq!(
            all_tabs(&doc),
            vec![
                TabRef::Graph(GraphRef::Main),
                TabRef::Preferences,
                TabRef::ImageViewer(out_port(node_id))
            ]
        );
        assert_eq!(doc.layout.primary().active, 2);
    }

    #[test]
    fn ensure_valid_layout_prunes_a_viewer_tab_whose_node_is_gone() {
        let mut doc = Document::default();
        let node_id = add_node_at(&mut doc, Vec2::ZERO);
        let primary = doc.layout.primary().id;
        doc.layout
            .find_or_insert(TabRef::ImageViewer(out_port(node_id)), primary);
        doc.ensure_valid_layout();
        assert_eq!(
            all_tabs(&doc).len(),
            2,
            "tab survives while the node exists"
        );

        // Delete the node: the viewer tab dies with it (like a graph
        // tab whose def vanished).
        doc.scope_mut(GraphRef::Main).unwrap().remove_node(&node_id);
        let err = doc.validate().unwrap_err();
        assert!(
            format!("{err:#}").contains("open tab references a missing target"),
            "unexpected validation error: {err:#}"
        );
        doc.ensure_valid_layout();
        assert_eq!(all_tabs(&doc), vec![TabRef::Graph(GraphRef::Main)]);
        assert_eq!(doc.layout.primary().active, 0);
    }

    #[test]
    fn dock_layout_round_trips_as_json() {
        use crate::core::document::dock::{DockDrop, SplitSide};

        let mut doc: Document = core_test_graph().into();
        let node_id = doc.graph.iter().next().unwrap().id;
        let primary = doc.layout.primary().id;
        doc.layout.find_or_insert(TabRef::Preferences, primary);
        doc.layout
            .find_or_insert(TabRef::ImageViewer(out_port(node_id)), primary);
        // A split pane too, so the whole tree shape round-trips — not
        // just a flat strip.
        doc.layout.move_tab(
            TabRef::ImageViewer(out_port(node_id)),
            DockDrop::Split {
                group: primary,
                side: SplitSide::Right,
            },
        );
        let bytes = serde_json::to_vec_pretty(&doc).expect("serialize with dock layout");
        let back: Document = serde_json::from_slice(&bytes).expect("deserialize");
        back.validate().expect("round-tripped document is valid");
        assert_eq!(
            back.layout, doc.layout,
            "the split tree (groups, focus, ratio) round-trips through JSON"
        );
    }

    #[test]
    fn document_passes_validate_debug() {
        let doc = build_test_doc();
        doc.validate_debug();
    }

    #[test]
    fn document_roundtrip() {
        let view = build_test_doc().main_view;
        let mut reordered = view.clone();
        let first_key = *reordered.item_placements.get_index(0).unwrap().0;
        let last_index = reordered.item_placements.len() - 1;
        reordered.move_item_to_index(&first_key, last_index);
        assert_ne!(view, reordered);

        assert_roundtrip();

        let mut invalid = build_test_doc();
        invalid.graph.origin = Some(GraphId::nil());
        let serialized = serde_json::to_vec(&invalid).unwrap();
        let invalid: Document = serde_json::from_slice(&serialized).unwrap();
        let error = invalid.validate().unwrap_err().to_string();
        assert!(error.contains("graph has a nil origin"), "{error}");

        let mut duplicate_bindings = serde_json::to_value(build_test_doc()).unwrap();
        let bindings = duplicate_bindings["graph"]["bindings"]
            .as_array_mut()
            .unwrap();
        bindings.push(bindings[0].clone());
        let serialized = serde_json::to_vec(&duplicate_bindings).unwrap();
        let error = serde_json::from_slice::<Document>(&serialized)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("duplicate binding for input port"),
            "{error}"
        );
    }

    #[test]
    #[should_panic(expected = "view item to move must exist")]
    fn moving_missing_view_item_panics() {
        GraphView::default().move_item_to_index(&ItemRef::Node(NodeId::unique()), 0);
    }

    fn build_test_doc() -> Document {
        core_test_graph().into()
    }

    fn assert_roundtrip() {
        let doc = build_test_doc();
        doc.validate_debug();
        let serialized = serde_json::to_vec_pretty(&doc).expect("serialize document");
        assert!(
            !serialized.is_empty(),
            "serialized document should not be empty"
        );
        let deserialized: Document = serde_json::from_slice(&serialized)
            .expect("document deserialization should succeed for test payload");
        deserialized
            .validate()
            .expect("deserialized document is valid");
        deserialized.validate_debug();
        assert_eq!(
            doc, deserialized,
            "the complete document should round-trip through JSON"
        );
    }

    #[test]
    fn validate_accepts_and_round_trips_a_pinned_output_with_its_item() {
        // A well-formed document — every pinned output carries a view item
        // (`for_graph` seeds one; the edit layer does the same on pin) —
        // validates and round-trips, position, slot, and all.
        let mut graph = core_test_graph();
        let node_id = graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id;
        let port = OutputPort::new(node_id, 0);
        graph.set_output_pinned(port, true);

        let mut doc: Document = graph.into();
        let key = ItemRef::Pin(port);
        let pos = Vec2::new(5.0, 6.0);
        *doc.main_view.item_placements.get_mut(&key).unwrap() = pos;
        doc.validate_debug();

        let bytes = serde_json::to_vec_pretty(&doc).expect("serialize");
        let reloaded: Document = serde_json::from_slice(&bytes).expect("load");
        reloaded.validate().expect("reloaded document is valid");
        assert_eq!(
            reloaded.main_view.item_placements.get(&key).copied(),
            Some(pos),
            "the pinned output's position round-trips"
        );
        assert_eq!(
            reloaded.main_view.item_placements.get_index_of(&key),
            doc.main_view.item_placements.get_index_of(&key),
            "the pinned output's paint-stack slot round-trips"
        );
    }

    #[test]
    fn validate_rejects_pin_item_drift_in_both_directions() {
        // A pinned output with no view item is malformed: the edit layer's
        // `MoveSelection` build looks the item up unconditionally, so validation
        // surfaces the drift rather than letting it crash later. Pin the
        // port *after* the view was built so nothing seeds the item.
        let graph = core_test_graph();
        let port = OutputPort::new(
            graph.find_by_name("sum", NodeSearch::TopLevel).unwrap().id,
            0,
        );
        let mut doc: Document = graph.into();
        doc.graph.set_output_pinned(port, true);
        let err = doc.validate().unwrap_err();
        assert!(
            format!("{err:#}").contains("pinned output must have a view item"),
            "unexpected validation error: {err:#}"
        );

        // The project loader calls this same gate in every build (release too),
        // so encoding with bare serde cannot make malformed state valid.
        let bytes = serde_json::to_vec(&doc).expect("serialize");
        let decoded: Document = serde_json::from_slice(&bytes).expect("deserialize");
        let err = decoded.validate().unwrap_err();
        assert!(
            format!("{err:#}").contains("pinned output must have a view item"),
            "unexpected deserialize error: {err:#}"
        );

        // The reverse drift — a ghost item for an unpinned port (unpinning
        // removes the item; a leftover would be a phantom slot in the paint
        // stack) — is rejected too.
        doc.graph.set_output_pinned(port, false);
        doc.main_view
            .item_placements
            .insert(ItemRef::Pin(port), Vec2::ZERO);
        let err = doc.validate().unwrap_err();
        assert!(
            format!("{err:#}").contains("view item references an output that isn't pinned"),
            "unexpected validation error: {err:#}"
        );
    }
}
