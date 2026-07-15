pub(crate) mod auto_layout;
pub(crate) mod dock;
pub(crate) mod view_item;

use anyhow::{Context, Result, bail, ensure};
use common::{KeyIndexVec, SerdeFormat, is_debug};
use glam::Vec2;
use scenarium::Library;
use scenarium::SubgraphRef;
use scenarium::{DetachedNode, Graph as CoreGraph, NodeId, NodeSearch, OutputPort};
use scenarium::{Node, NodeKind};
use scenarium::{SubgraphDef, SubgraphId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use thiserror::Error;

use crate::core::document::auto_layout::AUTO_LAYOUT_ORIGIN;
use crate::core::document::dock::DockLayout;
use crate::core::document::view_item::ViewItem;
use crate::core::edit::reconcile::reconcile_def;

/// Initial placement of a fresh subgraph's boundary nodes: the input
/// boundary at the origin, the output boundary one gap to the right and
/// level with it (instead of the generic auto-layout stacking the two
/// unconnected nodes in one column).
const BOUNDARY_LAYOUT_GAP: f32 = 520.0;

/// The document is structurally invalid: a file read from disk is untrusted
/// input (hand-edited, corrupt, or stale against the editor), so
/// [`Document::check`] surfaces a violation as this recoverable error rather
/// than panicking. The load-path counterpart of scenarium's `CompileError`;
/// only `check` produces it.
#[derive(Debug, Error)]
#[error("invalid document: {message}")]
pub(crate) struct DocumentError {
    pub message: String,
}

/// Which graph an editor tab is pointed at. `Main` is the document's
/// root graph; `Local(id)` is a local subgraph def's interior graph
/// (`Document::graph.subgraphs[id].graph`). Linked subgraphs are shared
/// library assets in the `Library` — not editable in place; to edit one
/// you localize it (copy into the doc as a `Local` def) first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum GraphRef {
    Main,
    Local(SubgraphId),
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
/// the whole document (subgraph interiors included), so no graph ref is
/// needed alongside — upheld by `SubgraphDef::fresh_copy` at every
/// def-copy boundary (import/localize/detach) and enforced by
/// [`Document::check`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct PortRef {
    pub node_id: NodeId,
    pub kind: PortKind,
    pub port_idx: usize,
}

/// What an editor tab shows. Most tabs are graphs — the root and any
/// opened subgraph interiors ([`TabRef::Graph`]) — but a tab can also be
/// a non-graph app view like [`TabRef::Preferences`] (the settings window).
/// Persisted + undoable like the rest of the tab/view state, so reopening
/// a document restores its open tabs and Ctrl+Z walks tab open/close.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum TabRef {
    /// A graph pane (root or a local subgraph interior).
    Graph(GraphRef),
    /// The app-preferences / settings view — no graph, no canvas.
    Preferences,
    /// A full-resolution viewer of one port's runtime image — one tab per
    /// port, deduped on open. Content is runtime-only
    /// (`crate::gui::image_viewer`): a restored tab pulls any current value
    /// from `RunState` when drawn. Pruned when its node is deleted, like a
    /// subgraph tab whose def vanished.
    ImageViewer(PortRef),
}

/// Which side of a subgraph def's interface a boundary-port edit targets:
/// `Input` → `def.inputs` (the `SubgraphInput` node's output ports),
/// `Output` → `def.outputs` (the `SubgraphOutput` node's input ports).
///
/// The side names the *interface*, not the UI column it shows in: a
/// boundary node mirrors the interface, so the `SubgraphOutput` node's
/// *input column* edits the `Output` side and the `SubgraphInput` node's
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
/// (`GraphView::view_items`, keyed by this — `Intent::Raise` lifts either
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
    /// Zoom guarded against the degenerate `0` (a viewport that was
    /// never set), so inverse transforms can't divide by zero.
    pub(crate) fn safe_zoom(&self) -> f32 {
        if self.zoom > 0.0 { self.zoom } else { 1.0 }
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
/// open/edited graph (the root in `Document::main_view`, each subgraph
/// interior in `Document::sub_views`). The graph *data* itself lives in
/// the core `Graph`; this is purely how the editor presents and
/// navigates it.
///
/// **Everything here is persisted and undoable, by design** — reopening
/// a file restores the exact camera and selection, and Ctrl+Z walks
/// camera/selection changes alongside structural edits (see the long
/// note that used to live on `Document`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GraphView {
    /// Node bodies and pinned-output preview widgets in one list — the
    /// canvas's **paint stack**: the order is the shared z-order (later
    /// items draw in front), so a preview can sit above or below any
    /// node independently, and `Intent::Raise` lifts either kind to the
    /// top. Exactly one `Node` item per graph node, exactly one `Pin`
    /// item per *currently pinned* output (unpinning removes the item;
    /// undo restores it, slot included) — enforced by [`Self::check`].
    ///
    /// A pin's position is absolute rather than port-relative: the
    /// widget sits where it was put and does *not* follow its node when
    /// that node is moved — only its wire re-routes, like a connection
    /// to another node would.
    pub view_items: KeyIndexVec<ItemRef, ViewItem>,
    pub viewport: Viewport,
    /// `BTreeSet` so equality and serialization are order-independent
    /// (no spurious undo entries from reordering). Holds both node bodies
    /// and pinned-output preview widgets — see [`ItemRef`].
    pub selected: BTreeSet<ItemRef>,
}

impl Eq for GraphView {}

impl GraphView {
    /// A fresh view seeded with a zero-positioned item for every node in
    /// `graph` and every pinned output (callers usually `auto_layout`
    /// right after, which places both kinds).
    pub(crate) fn for_graph(graph: &CoreGraph) -> Self {
        let mut view_items = KeyIndexVec::with_capacity(graph.len());
        for node in graph.iter() {
            view_items.add(ViewItem::node(node.id, Vec2::ZERO));
        }
        for port in graph.pinned_outputs() {
            view_items.add(ViewItem::pin(port, Vec2::ZERO));
        }
        Self {
            view_items,
            ..Default::default()
        }
    }

    fn check(&self, graph: &CoreGraph) -> Result<()> {
        ensure!(
            self.viewport.zoom.is_finite() && self.viewport.zoom > 0.0,
            "graph zoom must be finite and positive"
        );
        ensure!(self.viewport.pan.is_finite(), "graph pan must be finite");

        // Item-set match, both kinds. Duplicate keys are rejected at
        // deserialize and unrepresentable by construction (`KeyIndexVec`),
        // so per-kind count matches plus one-way containment prove set
        // equality: exactly one `Node` item per graph node, exactly one
        // `Pin` item per currently-pinned output. Pin items track the
        // pinned set exactly — the edit layer removes the item on unpin
        // (its undo step restores it), so a `Pin` item for an unpinned
        // port is corrupt input, and a pinned port without an item would
        // break the `MoveSelection` lookup in `build_step`.
        let mut node_items = 0usize;
        for item in self.view_items.iter() {
            ensure!(
                item.pos.is_finite(),
                "view item {:?} position must be finite",
                item.key
            );
            match item.key {
                ItemRef::Node(_) => node_items += 1,
                ItemRef::Pin(port) => ensure!(
                    graph.is_output_pinned(port),
                    "view item references an output that isn't pinned"
                ),
            }
        }
        ensure!(
            node_items == graph.len(),
            "view node items must match graph nodes"
        );
        for node in graph.iter() {
            ensure!(
                self.view_items.by_key(&ItemRef::Node(node.id)).is_some(),
                "graph view missing a position for node {:?}",
                node.id
            );
        }
        for port in graph.pinned_outputs() {
            ensure!(
                self.view_items.by_key(&ItemRef::Pin(port)).is_some(),
                "pinned output must have a view item"
            );
        }

        for key in &self.selected {
            ensure!(
                self.view_items.by_key(key).is_some(),
                "selected item {key:?} has no view item"
            );
        }
        Ok(())
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
            .view_items
            .retain(|item| !item.key.belongs_to(*node_id));
        let detached = self.graph.detach_node(*node_id);
        self.view.selected.retain(|k| !k.belongs_to(*node_id));
        detached
    }
}

/// The thing being edited: the core `Graph` (which already nests local
/// subgraph defs and their interior graphs) plus the editor view
/// metadata for each graph the user has open — the root in `main_view`,
/// every opened subgraph interior in `sub_views`. The `Library` it
/// resolves against lives one level up on `App` (runtime-owned).
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Document {
    pub graph: CoreGraph,
    pub main_view: GraphView,
    /// View metadata for local subgraph interiors, created lazily when a
    /// subgraph is first opened in a tab. Keyed by `SubgraphId`.
    #[serde(default)]
    pub sub_views: HashMap<SubgraphId, GraphView>,
    /// The pane arrangement: open tabs grouped into split panes, plus
    /// the focused group. Persisted + undoable like the rest of the view
    /// state (every layout mutation is an undoable `Intent::Dock`).
    #[serde(default)]
    pub layout: DockLayout,
}

/// Whether a tab still resolves against the graph: `Main` and
/// `Preferences` always do, a subgraph tab lives with its def, and a
/// viewer tab dies with its node (mirroring a subgraph tab whose def
/// vanished). The single predicate behind
/// [`Document::ensure_valid_layout`]'s fast-path *and* its prune, so
/// the two can't drift.
fn tab_alive(graph: &CoreGraph, tab: TabRef) -> bool {
    match tab {
        TabRef::Graph(GraphRef::Main) | TabRef::Preferences => true,
        TabRef::Graph(GraphRef::Local(id)) => graph.subgraphs.by_key(&id).is_some(),
        TabRef::ImageViewer(port) => graph
            .find_node(&port.node_id, NodeSearch::Recursive)
            .is_some(),
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
    /// (e.g. a subgraph deleted while its tab was open).
    pub(crate) fn graph_for(&self, target: GraphRef) -> Option<&CoreGraph> {
        match target {
            GraphRef::Main => Some(&self.graph),
            GraphRef::Local(id) => self.graph.subgraphs.by_key(&id).map(|d| &d.graph),
        }
    }

    /// The view metadata for a target, or `None` when unopened/missing.
    pub(crate) fn view(&self, target: GraphRef) -> Option<&GraphView> {
        match target {
            GraphRef::Main => Some(&self.main_view),
            GraphRef::Local(id) => self.sub_views.get(&id),
        }
    }

    /// Mutable graph for a target — the root graph or a local subgraph
    /// interior. Unlike `scope_mut` this hands back only the graph (no
    /// view), so callers that rewire bindings across *several* graphs in
    /// one pass (e.g. boundary reconcile remapping instance bindings) can
    /// borrow each in turn without dragging the view along.
    pub(crate) fn graph_mut(&mut self, target: GraphRef) -> Option<&mut CoreGraph> {
        match target {
            GraphRef::Main => Some(&mut self.graph),
            GraphRef::Local(id) => self.graph.subgraphs.by_key_mut(&id).map(|d| &mut d.graph),
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
                let def = self.graph.subgraphs.by_key_mut(&id)?;
                let view = self.sub_views.get_mut(&id)?;
                Some(EditScope {
                    graph: &mut def.graph,
                    view,
                })
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

    /// Ensure a `GraphView` exists for a local subgraph interior,
    /// auto-laying-out its nodes on first creation. Returns `false` if
    /// the subgraph no longer exists.
    pub(crate) fn ensure_sub_view(&mut self, id: SubgraphId) -> bool {
        if self.sub_views.contains_key(&id) {
            return true;
        }
        let view = {
            let Some(def) = self.graph.subgraphs.by_key(&id) else {
                return false;
            };
            let mut view = GraphView::for_graph(&def.graph);
            view.auto_layout(&def.graph);
            view
        };
        self.sub_views.insert(id, view);
        true
    }

    /// Keep the layout renderable: drop tabs whose graph vanished
    /// (collapsing panes that empty) and seed any `Local` tab that's
    /// missing its view metadata — so the scene rebuild always resolves
    /// a live graph *and* view. `Main` always survives (`graph_for(Main)`
    /// is infallible and `main_view` always exists).
    ///
    /// The view-seeding covers a desync hazard: the layout and
    /// `sub_views` are independent serialized fields, so a deserialized
    /// (or hand-edited) document can carry a `Local` tab with no matching
    /// `sub_views` entry. Seeding it here recovers gracefully instead of
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
            |t| matches!(t, TabRef::Graph(GraphRef::Local(id)) if !self.sub_views.contains_key(&id)),
        ) {
            let missing: Vec<SubgraphId> = self
                .layout
                .all_tabs()
                .filter_map(|t| match t {
                    TabRef::Graph(GraphRef::Local(id)) if !self.sub_views.contains_key(&id) => {
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

    /// Add an imported subgraph `def` to this document's local defs,
    /// returning its assigned id. The copy takes fresh identity end to
    /// end via [`SubgraphDef::fresh_copy`]: a fresh def id so an import
    /// never overwrites an existing def, and fresh interior node ids —
    /// nested defs included — so re-importing the same file can't break
    /// the document-wide node-id uniqueness [`PortRef`] relies on
    /// (nested def ids are level-scoped and ride along unchanged).
    /// `origin` is carried over so a re-imported asset keeps its library
    /// lineage for Publish. The undo stack is unaffected: no existing
    /// history references the freshly added def.
    pub(crate) fn import_subgraph(&mut self, def: SubgraphDef) -> SubgraphId {
        let mut copy = def.fresh_copy();
        copy.origin = def.origin;
        let id = copy.id;
        self.graph.subgraphs.add(copy);
        id
    }

    /// Create a fresh, empty local subgraph — just the two boundary nodes
    /// (`SubgraphInput`/`SubgraphOutput`), no interface yet (it's derived
    /// from interior wiring, so an unwired pair exposes nothing). Returns
    /// the new id for the caller to open in a tab.
    pub(crate) fn create_subgraph(&mut self) -> SubgraphId {
        let mut graph = CoreGraph::default();
        let input = Node::new(NodeKind::SubgraphInput);
        let output = Node::new(NodeKind::SubgraphOutput);
        let (input_id, output_id) = (input.id, output.id);
        graph.add(input);
        graph.add(output);
        let def = SubgraphDef::new(
            SubgraphId::unique(),
            format!("subgraph {}", self.graph.subgraphs.len() + 1),
        )
        .graph(graph);
        let id = def.id;
        // Drop an instance of the new subgraph into the root graph,
        // staggered so repeated creates don't perfectly overlap. Built
        // before the def moves into the table (needs `&def`); the empty
        // interface means no input ports to seed.
        let inst = Node::subgraph_instance(&def, SubgraphRef::Local(id));
        let inst_id = inst.id;
        let inst_pos =
            Vec2::new(60.0, 60.0) + Vec2::splat(36.0) * self.graph.subgraphs.len() as f32;
        self.graph.subgraphs.add(def);
        self.graph.add(inst);
        self.main_view
            .view_items
            .add(ViewItem::node(inst_id, inst_pos));

        // Seed the interior view explicitly so the pair opens input-left /
        // output-right; `ensure_sub_view` then finds it and skips the
        // generic auto-layout that would stack them.
        let mut view = GraphView::default();
        view.view_items
            .add(ViewItem::node(input_id, AUTO_LAYOUT_ORIGIN));
        view.view_items.add(ViewItem::node(
            output_id,
            AUTO_LAYOUT_ORIGIN + Vec2::new(BOUNDARY_LAYOUT_GAP, 0.0),
        ));
        self.sub_views.insert(id, view);

        id
    }

    /// Current name of a subgraph interface port (`inputs[idx]` for
    /// `Input`, `outputs[idx]` for `Output`), or `None` if the def /
    /// side / index doesn't resolve.
    pub(crate) fn boundary_port_name(
        &self,
        sub_id: SubgraphId,
        side: BoundarySide,
        idx: usize,
    ) -> Option<&str> {
        let def = self.graph.subgraphs.by_key(&sub_id)?;
        let name = match side {
            BoundarySide::Input => &def.inputs.get(idx)?.name,
            BoundarySide::Output => &def.outputs.get(idx)?.name,
        };
        Some(name)
    }

    /// Rename the interface port currently named `expected` on `side` to
    /// `new`. `idx_hint` is tried first (exact when nothing moved, and it
    /// disambiguates duplicate names); otherwise the slot is found by its
    /// `expected` name. Resolving by name lets undo/redo survive
    /// `reconcile_boundaries` compacting the interface — it renumbers
    /// indices but *preserves names*, so the renamed slot is still found
    /// at its new index. No-op if no matching slot exists (e.g. the port
    /// was disconnected away entirely).
    pub(crate) fn rename_boundary_port(
        &mut self,
        sub_id: SubgraphId,
        side: BoundarySide,
        idx_hint: usize,
        expected: &str,
        new: &str,
    ) {
        let Some(def) = self.graph.subgraphs.by_key_mut(&sub_id) else {
            return;
        };
        let slot = match side {
            BoundarySide::Input => resolve_named_slot(&def.inputs, idx_hint, expected, |i| &i.name)
                .map(|i| &mut def.inputs[i].name),
            BoundarySide::Output => {
                resolve_named_slot(&def.outputs, idx_hint, expected, |o| &o.name)
                    .map(|i| &mut def.outputs[i].name)
            }
        };
        if let Some(slot) = slot {
            *slot = new.to_owned();
        }
    }

    /// Reconcile every local subgraph def's interface (`inputs`/`outputs`)
    /// against its interior wiring — derived state, recomputed like the
    /// scene rather than stored as undo steps. See `crate::core::edit::reconcile` for
    /// the per-def logic and rationale (placeholder ports, compaction).
    pub(crate) fn reconcile_boundaries(&mut self, library: &Library) {
        if self.graph.subgraphs.is_empty() {
            return;
        }
        let def_ids: Vec<SubgraphId> = self.graph.subgraphs.iter().map(|d| d.id).collect();
        for id in def_ids {
            reconcile_def(self, id, library);
        }
    }

    /// Drop wiring left dangling when a node's func/def changed its interface
    /// (e.g. a document loaded against a newer library): data bindings whose
    /// port is now out of range, and event subscriptions whose event is gone.
    /// Derived-validity fixup run alongside [`Self::reconcile_boundaries`];
    /// both recurse into local subgraph defs.
    pub(crate) fn prune_dangling_wiring(&mut self, library: &Library) {
        self.graph.prune_dangling_wiring(library);
    }

    /// Full structural validation, in all builds. A document read from disk
    /// is untrusted input, so a violation is a recoverable [`DocumentError`]
    /// the caller surfaces — not a panic. The debug-only assert form for
    /// documents the editor itself built is [`Self::validate`].
    pub(crate) fn check(&self) -> Result<(), DocumentError> {
        self.check_inner().map_err(|e| DocumentError {
            message: format!("{e:#}"),
        })
    }

    /// The anyhow-backed body of [`Self::check`], kept separate so the
    /// individual checks compose with `ensure!`/`context` and only the
    /// boundary converts to the typed error.
    fn check_inner(&self) -> Result<()> {
        self.graph.check()?;

        // Node ids must be unique across the whole document, def interiors
        // included: `PortRef` carries no graph ref, and run state, inspectors,
        // and widget ids all key nodes by bare `NodeId`. Every def-copy
        // boundary (import / localize / detach) severs identity via
        // `SubgraphDef::fresh_copy`, so a duplicate is corrupt input.
        fn collect_node_ids(graph: &CoreGraph, seen: &mut HashSet<NodeId>) -> Result<()> {
            for node in graph.iter() {
                ensure!(
                    seen.insert(node.id),
                    "node id {:?} appears in more than one graph",
                    node.id
                );
            }
            for def in graph.subgraphs.iter() {
                collect_node_ids(&def.graph, seen)?;
            }
            Ok(())
        }
        collect_node_ids(&self.graph, &mut HashSet::new())?;

        self.main_view.check(&self.graph).context("main view")?;

        // Each opened subgraph view must match its interior graph; a
        // view whose subgraph was deleted is a stale entry.
        for (id, view) in &self.sub_views {
            let def = self.graph.subgraphs.by_key(id).with_context(|| {
                format!("sub_views entry references missing local subgraph {id:?}")
            })?;
            view.check(&def.graph)
                .with_context(|| format!("subgraph {id:?} view"))?;
        }

        self.layout.check()?;
        for tab in self.layout.all_tabs() {
            if let TabRef::Graph(g) = tab {
                ensure!(
                    self.graph_for(g).is_some(),
                    "open tab references a missing graph {g:?}"
                );
            }
        }
        Ok(())
    }

    /// Debug-only assert form of [`Self::check`]: a violation in a document
    /// the editor itself built is our bug, caught loudly in development and
    /// free in release. Untrusted (deserialized) documents go through `check`
    /// in every build.
    pub(crate) fn validate(&self) {
        if !is_debug() {
            return;
        }
        if let Err(err) = self.check() {
            panic!("{err}");
        }
    }

    pub(crate) fn serialize(&self, format: SerdeFormat) -> Result<Vec<u8>> {
        self.validate();
        common::serialize(self, format)
    }

    pub(crate) fn deserialize(format: SerdeFormat, input: &[u8]) -> Result<Self> {
        if input.is_empty() {
            bail!("document input is empty");
        }

        let doc = common::deserialize::<Document>(input, format)?;
        doc.check()?;

        Ok(doc)
    }
}

impl Eq for Document {}

impl From<CoreGraph> for Document {
    fn from(graph: CoreGraph) -> Self {
        let main_view = GraphView::for_graph(&graph);
        Self {
            graph,
            main_view,
            sub_views: HashMap::new(),
            layout: DockLayout::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::FuncId;
    use scenarium::testing::test_graph as core_test_graph;

    /// A childless local def with the given id/name.
    fn leaf_def(id: SubgraphId, name: &str) -> SubgraphDef {
        SubgraphDef::new(id, name)
    }

    #[test]
    fn import_regenerates_ids_and_keeps_nested_def_ids() {
        // Real storage shape: a child def lives in its *parent's* interior
        // `graph.subgraphs`, instanced by an interior node — not in a flat
        // root table. Importing the parent carries the child with it.
        let child_id = SubgraphId::unique();
        let parent_id = SubgraphId::unique();
        let origin_id = SubgraphId::unique();
        let mut interior = CoreGraph::default();
        interior.subgraphs.add(leaf_def(child_id, "child"));
        interior.add(Node::new(NodeKind::Subgraph(SubgraphRef::Local(child_id))));
        let parent = SubgraphDef::new(parent_id, "parent")
            .graph(interior)
            .origin(origin_id);
        let source_ids: Vec<NodeId> = parent.graph.iter().map(|n| n.id).collect();

        let mut doc = Document::default();
        let id_a = doc.import_subgraph(parent.clone());
        let id_b = doc.import_subgraph(parent);

        assert_ne!(id_a, parent_id, "top-level id is regenerated");
        assert_ne!(id_a, id_b, "each import is its own def");
        assert!(
            doc.graph.subgraphs.by_key(&parent_id).is_none(),
            "original top id is not reused"
        );
        let interior_ids = |id: SubgraphId| -> Vec<NodeId> {
            let def = doc.graph.subgraphs.by_key(&id).expect("def resolves");
            assert_eq!(
                def.origin,
                Some(origin_id),
                "library lineage is carried over"
            );
            // The nested child def rides along under its (level-scoped) id.
            assert_eq!(def.graph.subgraphs.len(), 1);
            assert!(
                def.graph.subgraphs.by_key(&child_id).is_some(),
                "nested child def is preserved with its original id"
            );
            def.graph.iter().map(|n| n.id).collect()
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
        doc.check().unwrap();
    }

    #[test]
    fn check_rejects_duplicate_node_ids_across_graphs() {
        // The same node id planted in the root graph and a def interior —
        // unreachable through the editor (import/localize/detach sever
        // identity via `fresh_copy`), so it's corrupt input `check` refuses.
        let mut doc = Document::default();
        let node_id = add_node_at(&mut doc, Vec2::ZERO);
        let sub_id = doc.create_subgraph();
        let mut dup = Node::new(NodeKind::Func(FuncId::unique()));
        dup.id = node_id;
        doc.graph
            .subgraphs
            .by_key_mut(&sub_id)
            .unwrap()
            .graph
            .add(dup);

        let err = doc.check().unwrap_err();
        assert!(
            err.message
                .contains("occurs in more than one authoring graph"),
            "unexpected check error: {err}"
        );
    }

    #[test]
    fn importing_same_def_twice_makes_two_copies() {
        let id = SubgraphId::unique();
        let mut doc = Document::default();
        let a = doc.import_subgraph(leaf_def(id, "x"));
        let b = doc.import_subgraph(leaf_def(id, "x"));
        assert_ne!(a, b, "each import gets its own id");
        assert_eq!(doc.graph.subgraphs.len(), 2, "no silent overwrite");
    }

    #[test]
    fn add_node_with_def_round_trips() {
        use crate::core::edit::intent::apply::{apply_step, revert_step};
        use crate::core::edit::intent::build::build_step;
        use crate::core::edit::intent::types::Intent;

        // Instancing a library subgraph localizes it: a `Local` def copy
        // (recording its `origin`) is added alongside the instance node, as
        // one undoable `AddNode`.
        let lib_id = SubgraphId::unique();
        let mut local = leaf_def(lib_id, "Lib").fresh_copy();
        local.origin = Some(lib_id);
        let local_id = local.id;
        let node = Node::subgraph_instance(&local, SubgraphRef::Local(local_id));
        let node_id = node.id;

        let mut doc = Document::default();
        let step = build_step(
            Intent::AddNode {
                pos: Vec2::ZERO,
                node,
                def: Some(Box::new(local)),
                bindings: vec![],
            },
            &doc,
            GraphRef::Main,
        )
        .expect("add builds");

        apply_step(&step, &mut doc, GraphRef::Main);
        assert!(
            doc.graph.subgraphs.by_key(&local_id).is_some(),
            "local def added alongside the instance"
        );
        assert_eq!(
            doc.graph.subgraphs.by_key(&local_id).unwrap().origin,
            Some(lib_id),
            "copy records its library origin"
        );
        assert!(
            doc.graph
                .find_node(&node_id, NodeSearch::TopLevel)
                .is_some(),
            "instance node added"
        );

        revert_step(&step, &mut doc, GraphRef::Main);
        assert!(
            doc.graph.subgraphs.by_key(&local_id).is_none(),
            "undo removes the def"
        );
        assert!(
            doc.graph
                .find_node(&node_id, NodeSearch::TopLevel)
                .is_none(),
            "undo removes the instance node"
        );
    }

    /// Localize one library instance into `doc`'s root graph and return
    /// `(node_id, local_def_id)`. `origin` tags the copy's library
    /// lineage so a later instance can dedup against it.
    fn add_library_instance(doc: &mut Document, lib_id: SubgraphId) -> (NodeId, SubgraphId) {
        use crate::core::edit::intent::apply::apply_step;
        use crate::core::edit::intent::build::build_step;
        use crate::core::edit::intent::types::Intent;

        let mut local = leaf_def(lib_id, "Lib").fresh_copy();
        local.origin = Some(lib_id);
        let local_id = local.id;
        let node = Node::subgraph_instance(&local, SubgraphRef::Local(local_id));
        let node_id = node.id;
        let step = build_step(
            Intent::AddNode {
                pos: Vec2::ZERO,
                node,
                def: Some(Box::new(local)),
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
        // Two instances of the same library subgraph dropped into one
        // graph must share a single local def: the first materializes the
        // localized copy, the second re-points at it (no duplicate def).
        let lib_id = SubgraphId::unique();
        let mut doc = Document::default();

        let (_node_a, def_a_id) = add_library_instance(&mut doc, lib_id);
        assert_eq!(doc.graph.subgraphs.len(), 1, "first instance adds the def");

        let (node_b, def_b_id) = add_library_instance(&mut doc, lib_id);
        assert_eq!(
            doc.graph.subgraphs.len(),
            1,
            "second instance reuses the def — no duplicate"
        );
        assert!(
            doc.graph.subgraphs.by_key(&def_b_id).is_none(),
            "the second fresh copy was dropped"
        );
        assert_eq!(
            doc.graph
                .find_node(&node_b, NodeSearch::TopLevel)
                .unwrap()
                .kind,
            NodeKind::Subgraph(SubgraphRef::Local(def_a_id)),
            "second instance points at the first instance's local def"
        );
    }

    #[test]
    fn detach_forks_standalone_copy_and_repoints_node() {
        use crate::core::edit::intent::apply::{apply_step, revert_step};
        use crate::core::edit::intent::build::build_step;
        use crate::core::edit::intent::types::Intent;

        // A node on a library-linked local def. Detach must fork a fresh
        // standalone copy (origin cleared), add it, and repoint the node.
        let lib_id = SubgraphId::unique();
        let mut doc = Document::default();
        let mut local = leaf_def(SubgraphId::unique(), "Lib");
        local.origin = Some(lib_id);
        let local_id = local.id;
        doc.graph.subgraphs.add(local);
        let node = Node::subgraph_instance(
            doc.graph.subgraphs.by_key(&local_id).unwrap(),
            SubgraphRef::Local(local_id),
        );
        let node_id = node.id;
        doc.graph.add(node);
        doc.main_view
            .view_items
            .add(ViewItem::node(node_id, Vec2::ZERO));

        let step = build_step(Intent::DetachSubgraph { node_id }, &doc, GraphRef::Main)
            .expect("detach builds");
        apply_step(&step, &mut doc, GraphRef::Main);

        assert_eq!(doc.graph.subgraphs.len(), 2, "fork adds a second local def");
        let NodeKind::Subgraph(SubgraphRef::Local(new_id)) = doc
            .graph
            .find_node(&node_id, NodeSearch::TopLevel)
            .unwrap()
            .kind
        else {
            panic!("node should still be a local subgraph");
        };
        assert_ne!(new_id, local_id, "node now points at the fork");
        assert_eq!(
            doc.graph.subgraphs.by_key(&new_id).unwrap().origin,
            None,
            "detach clears the library lineage"
        );

        revert_step(&step, &mut doc, GraphRef::Main);
        assert_eq!(doc.graph.subgraphs.len(), 1, "undo drops the fork");
        let NodeKind::Subgraph(SubgraphRef::Local(restored)) = doc
            .graph
            .find_node(&node_id, NodeSearch::TopLevel)
            .unwrap()
            .kind
        else {
            panic!("node should still be a local subgraph");
        };
        assert_eq!(restored, local_id, "undo restores the original ref");
    }

    #[test]
    fn instances_of_different_library_defs_stay_separate() {
        // Different library sources must NOT collapse into one local def.
        let mut doc = Document::default();
        add_library_instance(&mut doc, SubgraphId::unique());
        add_library_instance(&mut doc, SubgraphId::unique());
        assert_eq!(
            doc.graph.subgraphs.len(),
            2,
            "distinct library origins keep distinct local defs"
        );
    }

    /// Add a bare `Func`-kind node to `doc`'s root graph + main view at
    /// `pos`, returning its id.
    fn add_node_at(doc: &mut Document, pos: Vec2) -> NodeId {
        let node = Node::new(NodeKind::Func(FuncId::unique()));
        let id = node.id;
        doc.graph.add(node);
        doc.main_view.view_items.add(ViewItem::node(id, pos));
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
            !doc.graph
                .find_node(&id, NodeSearch::TopLevel)
                .unwrap()
                .disabled,
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
            doc.graph
                .find_node(&id, NodeSearch::TopLevel)
                .unwrap()
                .disabled,
            "apply disables"
        );

        revert_step(&step, &mut doc, GraphRef::Main);
        assert!(
            !doc.graph
                .find_node(&id, NodeSearch::TopLevel)
                .unwrap()
                .disabled,
            "revert re-enables (restores the captured `from`)"
        );
    }

    #[test]
    fn create_subgraph_has_only_boundary_nodes() {
        let mut doc = Document::default();
        let id = doc.create_subgraph();
        let def = doc.graph.subgraphs.by_key(&id).expect("def added");

        // Exactly the two boundary nodes, nothing else, empty interface.
        assert_eq!(def.graph.len(), 2);
        assert_eq!(
            def.graph
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::SubgraphInput))
                .count(),
            1
        );
        assert_eq!(
            def.graph
                .iter()
                .filter(|n| matches!(n.kind, NodeKind::SubgraphOutput))
                .count(),
            1
        );
        assert!(def.inputs.is_empty() && def.outputs.is_empty());

        // Boundary nodes are placed input-left / output-right, level.
        let input_id = def
            .graph
            .iter()
            .find(|n| matches!(n.kind, NodeKind::SubgraphInput))
            .unwrap()
            .id;
        let output_id = def
            .graph
            .iter()
            .find(|n| matches!(n.kind, NodeKind::SubgraphOutput))
            .unwrap()
            .id;
        let view = doc.sub_views.get(&id).expect("view seeded on create");
        let ip = view
            .view_items
            .by_key(&ItemRef::Node(input_id))
            .unwrap()
            .pos;
        let op = view
            .view_items
            .by_key(&ItemRef::Node(output_id))
            .unwrap()
            .pos;
        assert!(op.x > ip.x, "output boundary sits right of input");
        assert_eq!(ip.y, op.y, "boundaries are level");

        // Creating also drops an instance of the new subgraph into root.
        let inst = doc
            .graph
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Subgraph(SubgraphRef::Local(sid)) if sid == id))
            .expect("instance added to main graph");
        assert!(
            doc.main_view
                .view_items
                .by_key(&ItemRef::Node(inst.id))
                .is_some(),
            "instance has a main view item"
        );

        // Each create mints a distinct id (no overwrite).
        let id2 = doc.create_subgraph();
        assert_ne!(id, id2);
        assert_eq!(doc.graph.subgraphs.len(), 2);
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
        doc.validate();
    }

    #[test]
    fn ensure_valid_layout_keeps_non_graph_tabs_when_a_subgraph_tab_vanishes() {
        let mut doc = Document::default();
        let node_id = add_node_at(&mut doc, Vec2::ZERO);
        let id = doc.create_subgraph();
        let primary = doc.layout.primary().id;
        doc.layout
            .find_or_insert(TabRef::Graph(GraphRef::Local(id)), primary);
        doc.layout.find_or_insert(TabRef::Preferences, primary);
        doc.layout
            .find_or_insert(TabRef::ImageViewer(out_port(node_id)), primary);
        doc.layout.activate(primary, 3); // viewing the image tab
        // Drop the subgraph out from under its open tab.
        doc.graph.subgraphs.remove_by_key(&id);

        doc.ensure_valid_layout();
        // The dead subgraph tab is pruned; Main + the non-graph tabs
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

        // Delete the node: the viewer tab dies with it (like a subgraph
        // tab whose def vanished).
        doc.scope_mut(GraphRef::Main).unwrap().remove_node(&node_id);
        doc.ensure_valid_layout();
        assert_eq!(all_tabs(&doc), vec![TabRef::Graph(GraphRef::Main)]);
        assert_eq!(doc.layout.primary().active, 0);
    }

    #[test]
    fn dock_layout_round_trips_in_every_format() {
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
        for format in SerdeFormat::all_formats_for_testing() {
            let bytes = doc.serialize(format).expect("serialize with dock layout");
            let back = Document::deserialize(format, &bytes).expect("deserialize");
            assert_eq!(
                back.layout, doc.layout,
                "the split tree (groups, focus, ratio) round-trips for {format:?}"
            );
        }
    }

    #[test]
    fn document_validates() {
        let doc = build_test_doc();
        doc.validate();
    }

    #[test]
    fn document_roundtrip() {
        for format in SerdeFormat::all_formats_for_testing() {
            assert_roundtrip(format);
        }
    }

    fn build_test_doc() -> Document {
        core_test_graph().into()
    }

    fn assert_roundtrip(format: SerdeFormat) {
        let doc = build_test_doc();
        let serialized = doc.serialize(format).expect("serialize document");
        assert!(
            !serialized.is_empty(),
            "serialized document should not be empty"
        );
        let deserialized = Document::deserialize(format, &serialized)
            .expect("document deserialization should succeed for test payload");
        deserialized.validate();
        assert_eq!(
            doc.main_view.view_items.len(),
            deserialized.main_view.view_items.len(),
            "view item counts should round-trip"
        );
        assert_eq!(
            doc.main_view.view_items[0].key, deserialized.main_view.view_items[0].key,
            "view item keys should round-trip"
        );
        assert_eq!(
            doc.graph.len(),
            deserialized.graph.len(),
            "graph nodes should round-trip"
        );
        assert_eq!(
            doc.main_view.viewport.zoom, deserialized.main_view.viewport.zoom,
            "zoom should round-trip"
        );
        assert_eq!(
            doc.main_view.viewport.pan, deserialized.main_view.viewport.pan,
            "pan should round-trip"
        );
    }

    #[test]
    fn validate_accepts_and_round_trips_a_pinned_output_with_its_item() {
        // A well-formed document — every pinned output carries a view item
        // (`for_graph` seeds one; the edit layer does the same on pin) —
        // validates and round-trips, position, slot, and all.
        let mut graph = core_test_graph();
        let node_id = graph.by_name("sum").unwrap().id;
        let port = OutputPort::new(node_id, 0);
        graph.set_output_pinned(port, true);

        let mut doc: Document = graph.into();
        let key = ItemRef::Pin(port);
        let pos = Vec2::new(5.0, 6.0);
        doc.main_view.view_items.by_key_mut(&key).unwrap().pos = pos;
        doc.validate();

        let bytes = doc.serialize(SerdeFormat::Rhai).expect("serialize");
        let reloaded = Document::deserialize(SerdeFormat::Rhai, &bytes).expect("load");
        assert_eq!(
            reloaded.main_view.view_items.by_key(&key).map(|i| i.pos),
            Some(pos),
            "the pinned output's position round-trips"
        );
        assert_eq!(
            reloaded.main_view.view_items.index_of_key(&key),
            doc.main_view.view_items.index_of_key(&key),
            "the pinned output's paint-stack slot round-trips"
        );
    }

    #[test]
    fn check_rejects_pin_item_drift_in_both_directions() {
        // A pinned output with no view item is malformed: the edit layer's
        // `MoveSelection` build looks the item up unconditionally, so check
        // surfaces the drift rather than letting it crash later. Pin the
        // port *after* the view was built so nothing seeds the item.
        let graph = core_test_graph();
        let port = OutputPort::new(graph.by_name("sum").unwrap().id, 0);
        let mut doc: Document = graph.into();
        doc.graph.set_output_pinned(port, true);
        let err = doc.check().unwrap_err();
        assert!(
            err.message.contains("pinned output must have a view item"),
            "unexpected check error: {err}"
        );

        // The same gate guards deserialization in every build (release too):
        // encoding with bare serde bypasses `Document::serialize`'s debug
        // assert, and the load still refuses the malformed document.
        let bytes = common::serialize(&doc, SerdeFormat::Rhai).expect("serialize");
        let err = Document::deserialize(SerdeFormat::Rhai, &bytes).unwrap_err();
        assert!(
            format!("{err:#}").contains("pinned output must have a view item"),
            "unexpected deserialize error: {err:#}"
        );

        // The reverse drift — a ghost item for an unpinned port (unpinning
        // removes the item; a leftover would be a phantom slot in the paint
        // stack) — is rejected too.
        doc.graph.set_output_pinned(port, false);
        doc.main_view
            .view_items
            .add(ViewItem::pin(port, Vec2::ZERO));
        let err = doc.check().unwrap_err();
        assert!(
            err.message
                .contains("view item references an output that isn't pinned"),
            "unexpected check error: {err}"
        );
    }
}
