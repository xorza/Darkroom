pub(crate) mod auto_layout;
pub mod dock;
pub mod view_node;

use anyhow::{Context, Result, bail, ensure};
use common::{KeyIndexVec, SerdeFormat, is_debug};
use glam::Vec2;
use scenarium::graph::subgraph::SubgraphRef;
use scenarium::graph::subgraph::{SubgraphDef, SubgraphId};
use scenarium::graph::{Graph as CoreGraph, NodeId, NodeSearch, OutputPort};
use scenarium::graph::{Node, NodeKind};
use scenarium::library::Library;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use thiserror::Error;

use crate::core::document::auto_layout::AUTO_LAYOUT_ORIGIN;
use crate::core::document::dock::DockLayout;
use crate::core::document::view_node::ViewNode;
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
pub struct DocumentError {
    pub message: String,
}

/// Which graph an editor tab is pointed at. `Main` is the document's
/// root graph; `Local(id)` is a local subgraph def's interior graph
/// (`Document::graph.subgraphs[id].graph`). Linked subgraphs are shared
/// library assets in the `Library` — not editable in place; to edit one
/// you localize it (copy into the doc as a `Local` def) first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GraphRef {
    Main,
    Local(SubgraphId),
}

/// Whether a port consumes a binding (`Input`) or produces a value
/// (`Output`). Scoped to the data-port subset until Trigger/Event are
/// reintroduced. `Input` ports live in the left column, `Output` in
/// the right; `opposite` flips between them for snap-target tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PortKind {
    Input,
    Output,
}

impl PortKind {
    pub fn opposite(self) -> Self {
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
/// needed alongside.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PortRef {
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
pub enum TabRef {
    /// A graph pane (root or a local subgraph interior).
    Graph(GraphRef),
    /// The app-preferences / settings view — no graph, no canvas.
    Preferences,
    /// A full-resolution viewer of one port's runtime image — one tab per
    /// port, deduped on open. Content is runtime-only
    /// (`crate::gui::image_viewer`): a restored tab opens empty and fills
    /// itself after the next run. Pruned when its node is deleted, like a
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
pub enum BoundarySide {
    Input,
    Output,
}

/// A member of the selection set: either a node body or a pinned output's
/// floating preview widget. The two share one selection mechanism (click to
/// select, Shift-click to toggle membership, rubber-band to sweep both kinds
/// in) so `GraphView::selected` and everything downstream of it stay a
/// single `BTreeSet` rather than two parallel selection tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SelectionKey {
    Node(NodeId),
    Pin(OutputPort),
}

impl SelectionKey {
    /// Whether this key names something that lives on `node_id` — the node
    /// itself, or one of its pinned outputs. Used to prune a node's
    /// selection membership (both forms) when it's removed from the graph.
    fn belongs_to(self, node_id: NodeId) -> bool {
        match self {
            SelectionKey::Node(id) => id == node_id,
            SelectionKey::Pin(port) => port.node_id == node_id,
        }
    }
}

/// A graph's camera: pan offset (canvas-local px) + zoom factor. One
/// value shared by the persisted per-graph [`GraphView`], the per-frame
/// `Scene` projection, and the `SetViewport` edit, so the three can't
/// drift on field names or semantics.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub pan: Vec2,
    pub zoom: f32,
}

impl Viewport {
    /// Zoom guarded against the degenerate `0` (a viewport that was
    /// never set), so inverse transforms can't divide by zero.
    pub fn safe_zoom(&self) -> f32 {
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

/// Editor-side view metadata for one graph: per-node positions, the
/// viewport, and the selection. One of these exists per open/edited
/// graph (the root in `Document::main_view`, each subgraph interior in
/// `Document::sub_views`). The graph *data* itself lives in the core
/// `Graph`; this is purely how the editor presents and navigates it.
///
/// **Everything here is persisted and undoable, by design** — reopening
/// a file restores the exact camera and selection, and Ctrl+Z walks
/// camera/selection changes alongside structural edits (see the long
/// note that used to live on `Document`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphView {
    pub view_nodes: KeyIndexVec<NodeId, ViewNode>,
    pub viewport: Viewport,
    /// `BTreeSet` so equality and serialization are order-independent
    /// (no spurious undo entries from reordering). Holds both node bodies
    /// and pinned-output preview widgets — see [`SelectionKey`].
    pub selected: BTreeSet<SelectionKey>,
    /// A pinned output's satellite position, in absolute canvas-world
    /// coordinates (its top-left corner). Seeded the moment a port is
    /// pinned (see `crate::gui::canvas::pin_ui::PinUi::apply` and
    /// `crate::gui::node::port_row`'s pin/unpin menu toggle — both resolve
    /// the port's current on-screen position and write it here right away,
    /// so a pinned port always has an explicit entry) and re-seeded fresh
    /// on every pin — no memory of where a since-unpinned satellite last
    /// sat. Absolute rather than port-relative: the widget sits where it
    /// was put and does *not* follow the node if it's moved — only its
    /// wire re-routes, like a connection to another node would. Unpinning
    /// leaves a stale entry rather than pruning it — harmless (only read
    /// while `pinned`), and it means undoing an unpin restores the exact
    /// prior position with no extra bookkeeping. Pruned only when the node
    /// itself is removed (see [`EditScope::remove_node`]). Serialized as a
    /// `(port, position)` sequence — struct keys aren't valid map keys in
    /// string-keyed formats (JSON/TOML/Rhai), same reasoning as
    /// `scenarium::graph`'s `binding_map_serde`.
    #[serde(default, with = "pin_position_serde")]
    pub pin_positions: BTreeMap<OutputPort, Vec2>,
}

mod pin_position_serde {
    use super::{BTreeMap, OutputPort, Vec2};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer>(
        map: &BTreeMap<OutputPort, Vec2>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        map.iter().collect::<Vec<_>>().serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<OutputPort, Vec2>, D::Error> {
        Ok(Vec::<(OutputPort, Vec2)>::deserialize(deserializer)?
            .into_iter()
            .collect())
    }
}

impl Eq for GraphView {}

impl GraphView {
    /// A fresh view seeded with a zero-positioned `ViewNode` for every
    /// node in `graph` (callers usually `auto_layout` right after).
    pub fn for_graph(graph: &CoreGraph) -> Self {
        let mut view_nodes = KeyIndexVec::with_capacity(graph.len());
        for node in graph.iter() {
            view_nodes.add(ViewNode::from(node));
        }
        Self {
            view_nodes,
            ..Default::default()
        }
    }

    fn check(&self, graph: &CoreGraph) -> Result<()> {
        ensure!(
            self.viewport.zoom.is_finite() && self.viewport.zoom > 0.0,
            "graph zoom must be finite and positive"
        );
        ensure!(self.viewport.pan.is_finite(), "graph pan must be finite");

        for node in self.view_nodes.iter() {
            ensure!(
                node.pos.is_finite(),
                "node {:?} position must be finite",
                node.id
            );
        }

        // Exact node-set match. Both sides are `KeyIndexVec`s — duplicate ids
        // are rejected at deserialize and unrepresentable by construction —
        // so a length match plus one-way containment proves set equality.
        ensure!(
            self.view_nodes.len() == graph.len(),
            "view node list must match graph nodes"
        );
        for node in graph.iter() {
            ensure!(
                self.view_nodes.by_key(&node.id).is_some(),
                "graph view missing a position for node {:?}",
                node.id
            );
        }

        for key in &self.selected {
            let owner = match key {
                SelectionKey::Node(id) => *id,
                SelectionKey::Pin(port) => port.node_id,
            };
            ensure!(
                self.view_nodes.by_key(&owner).is_some(),
                "selected node {:?} is absent from the graph",
                owner
            );
        }

        for (port, position) in &self.pin_positions {
            ensure!(
                position.is_finite(),
                "pin position on node {:?} must be finite",
                port.node_id
            );
            ensure!(
                graph
                    .find_node(&port.node_id, NodeSearch::TopLevel)
                    .is_some(),
                "pin position references a node absent from the graph"
            );
        }
        // Every pinned output must carry an explicit position: the edit layer
        // treats a missing one as a logic error (a `MoveSelection` over the pin
        // looks it up unconditionally in `build_step`). Pinning always seeds it,
        // so a document reaching here without one is malformed — surface it
        // rather than paper over it. The reverse isn't required: `pin_positions`
        // keeps stale entries for since-unpinned ports on purpose (undo restores
        // them), so a position without a matching pin is fine.
        for port in graph.pinned_outputs() {
            ensure!(
                self.pin_positions.contains_key(&port),
                "pinned output must have an explicit pin position"
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
pub struct EditScope<'a> {
    pub graph: &'a mut CoreGraph,
    pub view: &'a mut GraphView,
}

/// Read-only graph + view pair, for `build_step`'s pre-mutation reads.
pub struct EditScopeRef<'a> {
    pub graph: &'a CoreGraph,
    pub view: &'a GraphView,
}

impl EditScope<'_> {
    /// Drop a node from both the graph and its view (positions +
    /// selection + any pinned outputs' custom satellite offsets). Mirrors
    /// the old `Document::remove_node`.
    pub fn remove_node(&mut self, node_id: &NodeId) {
        self.view.view_nodes.retain(|node| node.id != *node_id);
        self.graph.remove_by_id(*node_id);
        self.view.selected.retain(|k| !k.belongs_to(*node_id));
        self.view
            .pin_positions
            .retain(|port, _| port.node_id != *node_id);
    }
}

/// The thing being edited: the core `Graph` (which already nests local
/// subgraph defs and their interior graphs) plus the editor view
/// metadata for each graph the user has open — the root in `main_view`,
/// every opened subgraph interior in `sub_views`. The `Library` it
/// resolves against lives one level up on `App` (runtime-owned).
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Document {
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
    pub fn graph_for(&self, target: GraphRef) -> Option<&CoreGraph> {
        match target {
            GraphRef::Main => Some(&self.graph),
            GraphRef::Local(id) => self.graph.subgraphs.by_key(&id).map(|d| &d.graph),
        }
    }

    /// The view metadata for a target, or `None` when unopened/missing.
    pub fn view(&self, target: GraphRef) -> Option<&GraphView> {
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
    pub fn graph_mut(&mut self, target: GraphRef) -> Option<&mut CoreGraph> {
        match target {
            GraphRef::Main => Some(&mut self.graph),
            GraphRef::Local(id) => self.graph.subgraphs.by_key_mut(&id).map(|d| &mut d.graph),
        }
    }

    /// Graph + view borrowed together for editing the given target.
    pub fn scope_mut(&mut self, target: GraphRef) -> Option<EditScope<'_>> {
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
    pub fn scope(&self, target: GraphRef) -> Option<EditScopeRef<'_>> {
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
    pub fn active_target(&self) -> Option<GraphRef> {
        match self.layout.primary().active_tab() {
            TabRef::Graph(target) => Some(target),
            TabRef::Preferences | TabRef::ImageViewer(_) => None,
        }
    }

    /// Ensure a `GraphView` exists for a local subgraph interior,
    /// auto-laying-out its nodes on first creation. Returns `false` if
    /// the subgraph no longer exists.
    pub fn ensure_sub_view(&mut self, id: SubgraphId) -> bool {
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
    pub fn ensure_valid_layout(&mut self) {
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
    /// returning its assigned id. The top-level id is regenerated so an
    /// import never overwrites an existing def. Nested child defs ride
    /// along inside `def.graph.subgraphs` (a `Graph` carries its own
    /// subgraph table) and resolve only within this def's interior, so
    /// their ids can't collide with the document's table — they're left
    /// untouched. The undo stack is unaffected: no existing history
    /// references the freshly added def.
    pub fn import_subgraph(&mut self, mut def: SubgraphDef) -> SubgraphId {
        def.id = SubgraphId::unique();
        let id = def.id;
        self.graph.subgraphs.add(def);
        id
    }

    /// Create a fresh, empty local subgraph — just the two boundary nodes
    /// (`SubgraphInput`/`SubgraphOutput`), no interface yet (it's derived
    /// from interior wiring, so an unwired pair exposes nothing). Returns
    /// the new id for the caller to open in a tab.
    pub fn create_subgraph(&mut self) -> SubgraphId {
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
        self.main_view.view_nodes.add(ViewNode {
            id: inst_id,
            pos: inst_pos,
        });

        // Seed the interior view explicitly so the pair opens input-left /
        // output-right; `ensure_sub_view` then finds it and skips the
        // generic auto-layout that would stack them.
        let mut view = GraphView::default();
        view.view_nodes.add(ViewNode {
            id: input_id,
            pos: AUTO_LAYOUT_ORIGIN,
        });
        view.view_nodes.add(ViewNode {
            id: output_id,
            pos: AUTO_LAYOUT_ORIGIN + Vec2::new(BOUNDARY_LAYOUT_GAP, 0.0),
        });
        self.sub_views.insert(id, view);

        id
    }

    /// Current name of a subgraph interface port (`inputs[idx]` for
    /// `Input`, `outputs[idx]` for `Output`), or `None` if the def /
    /// side / index doesn't resolve.
    pub fn boundary_port_name(
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
    pub fn rename_boundary_port(
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
    pub fn reconcile_boundaries(&mut self, library: &Library) {
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
    pub fn prune_dangling_wiring(&mut self, library: &Library) {
        self.graph.prune_dangling_wiring(library);
    }

    /// Full structural validation, in all builds. A document read from disk
    /// is untrusted input, so a violation is a recoverable [`DocumentError`]
    /// the caller surfaces — not a panic. The debug-only assert form for
    /// documents the editor itself built is [`Self::validate`].
    pub fn check(&self) -> Result<(), DocumentError> {
        self.check_inner().map_err(|e| DocumentError {
            message: format!("{e:#}"),
        })
    }

    /// The anyhow-backed body of [`Self::check`], kept separate so the
    /// individual checks compose with `ensure!`/`context` and only the
    /// boundary converts to the typed error.
    fn check_inner(&self) -> Result<()> {
        self.graph.check()?;
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
    pub fn validate(&self) {
        if !is_debug() {
            return;
        }
        if let Err(err) = self.check() {
            panic!("{err}");
        }
    }

    pub fn serialize(&self, format: SerdeFormat) -> Result<Vec<u8>> {
        self.validate();
        common::serialize(self, format)
    }

    pub fn deserialize(format: SerdeFormat, input: &[u8]) -> Result<Self> {
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
    use scenarium::node::function::FuncId;
    use scenarium::testing::test_graph as core_test_graph;

    /// A childless local def with the given id/name.
    fn leaf_def(id: SubgraphId, name: &str) -> SubgraphDef {
        SubgraphDef::new(id, name)
    }

    #[test]
    fn import_regenerates_top_id_and_keeps_nested_defs() {
        // Real storage shape: a child def lives in its *parent's* interior
        // `graph.subgraphs`, instanced by an interior node — not in a flat
        // root table. Importing the parent carries the child with it.
        let child_id = SubgraphId::unique();
        let parent_id = SubgraphId::unique();
        let mut interior = CoreGraph::default();
        interior.subgraphs.add(leaf_def(child_id, "child"));
        interior.add(Node::new(NodeKind::Subgraph(SubgraphRef::Local(child_id))));
        let parent = SubgraphDef::new(parent_id, "parent").graph(interior);

        let mut doc = Document::default();
        let new_id = doc.import_subgraph(parent);

        assert_ne!(new_id, parent_id, "top-level id is regenerated");
        assert!(
            doc.graph.subgraphs.by_key(&parent_id).is_none(),
            "original top id is not reused"
        );
        let imported = doc
            .graph
            .subgraphs
            .by_key(&new_id)
            .expect("def resolves under its new id");
        // The nested child rides along untouched inside the interior table.
        assert_eq!(imported.graph.subgraphs.len(), 1);
        assert!(
            imported.graph.subgraphs.by_key(&child_id).is_some(),
            "nested child def is preserved with its original id"
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
        let view_node = ViewNode {
            id: node_id,
            pos: Vec2::ZERO,
        };

        let mut doc = Document::default();
        let step = build_step(
            Intent::AddNode {
                view_node,
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
                view_node: ViewNode {
                    id: node_id,
                    pos: Vec2::ZERO,
                },
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
        doc.main_view.view_nodes.add(ViewNode {
            id: node_id,
            pos: Vec2::ZERO,
        });

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
        doc.main_view.view_nodes.add(ViewNode { id, pos });
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
        let ip = view.view_nodes.by_key(&input_id).unwrap().pos;
        let op = view.view_nodes.by_key(&output_id).unwrap().pos;
        assert!(op.x > ip.x, "output boundary sits right of input");
        assert_eq!(ip.y, op.y, "boundaries are level");

        // Creating also drops an instance of the new subgraph into root.
        let inst = doc
            .graph
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Subgraph(SubgraphRef::Local(sid)) if sid == id))
            .expect("instance added to main graph");
        assert!(
            doc.main_view.view_nodes.by_key(&inst.id).is_some(),
            "instance has a main view node"
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
            doc.main_view.view_nodes.len(),
            deserialized.main_view.view_nodes.len(),
            "node view counts should round-trip"
        );
        assert_eq!(
            doc.main_view.view_nodes[0].id, deserialized.main_view.view_nodes[0].id,
            "node view ids should round-trip"
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
    fn validate_accepts_and_round_trips_a_pinned_output_with_its_position() {
        // A well-formed document — every pinned output carries a position, the
        // way the edit layer seeds it on pin — validates and round-trips with
        // no repair pass.
        let mut graph = core_test_graph();
        let node_id = graph.by_name("sum").unwrap().id;
        let port = OutputPort::new(node_id, 0);
        graph.set_output_pinned(port, true);

        let mut doc: Document = graph.into();
        let pos = Vec2::new(5.0, 6.0);
        doc.main_view.pin_positions.insert(port, pos);
        doc.validate();

        let bytes = doc.serialize(SerdeFormat::Rhai).expect("serialize");
        let reloaded = Document::deserialize(SerdeFormat::Rhai, &bytes).expect("load");
        assert_eq!(
            reloaded.main_view.pin_positions.get(&port),
            Some(&pos),
            "the pinned output's position round-trips"
        );
    }

    #[test]
    fn check_rejects_a_pinned_output_without_a_position() {
        // A pinned output with no matching pin position is malformed: the edit
        // layer's `MoveSelection` build looks the position up unconditionally,
        // so check surfaces the drift rather than letting it crash later.
        let mut graph = core_test_graph();
        let port = OutputPort::new(graph.by_name("sum").unwrap().id, 0);
        graph.set_output_pinned(port, true);
        let doc: Document = graph.into(); // no position — nothing auto-seeds one
        let err = doc.check().unwrap_err();
        assert!(
            err.message
                .contains("pinned output must have an explicit pin position"),
            "unexpected check error: {err}"
        );

        // The same gate guards deserialization in every build (release too):
        // encoding with bare serde bypasses `Document::serialize`'s debug
        // assert, and the load still refuses the malformed document.
        let bytes = common::serialize(&doc, SerdeFormat::Rhai).expect("serialize");
        let err = Document::deserialize(SerdeFormat::Rhai, &bytes).unwrap_err();
        assert!(
            format!("{err:#}").contains("pinned output must have an explicit pin position"),
            "unexpected deserialize error: {err:#}"
        );
    }
}
