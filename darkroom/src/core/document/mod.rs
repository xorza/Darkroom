pub mod view_node;

use anyhow::{Result, bail};
use common::{KeyIndexVec, SerdeFormat, is_debug};
use glam::Vec2;
use scenarium::graph::{Node, NodeKind};
use scenarium::prelude::{Graph as CoreGraph, Library, NodeId, SubgraphDef, SubgraphId};
use scenarium::subgraph::SubgraphRef;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

use crate::core::document::view_node::ViewNode;
use crate::core::edit::reconcile::reconcile_def;

/// Default topological-column auto-layout parameters, shared by every
/// place that seeds a fresh view (the root on load, each subgraph
/// interior on first open). Kept in one spot so the seeding paths can't
/// drift. The boundary placement below reuses `AUTO_LAYOUT_ORIGIN`.
const AUTO_LAYOUT_COL_SPACING: f32 = 220.0;
const AUTO_LAYOUT_ROW_SPACING: f32 = 110.0;
const AUTO_LAYOUT_ORIGIN: Vec2 = Vec2::new(40.0, 40.0);

/// Initial placement of a fresh subgraph's boundary nodes: the input
/// boundary at the origin, the output boundary one gap to the right and
/// level with it (instead of the generic auto-layout stacking the two
/// unconnected nodes in one column).
const BOUNDARY_LAYOUT_GAP: f32 = 520.0;

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

/// What an editor tab shows. Most tabs are graphs — the root and any
/// opened subgraph interiors ([`TabRef::Graph`]) — but a tab can also be
/// a non-graph app view like [`TabRef::Config`] (the settings window).
/// Persisted + undoable like the rest of the tab/view state, so reopening
/// a document restores its open tabs and Ctrl+Z walks tab open/close.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TabRef {
    /// A graph pane (root or a local subgraph interior).
    Graph(GraphRef),
    /// The app-config / settings view — no graph, no canvas.
    Config,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphView {
    pub view_nodes: KeyIndexVec<NodeId, ViewNode>,
    pub pan: Vec2,
    pub scale: f32,
    /// `BTreeSet` so equality and serialization are order-independent
    /// (no spurious undo entries from reordering).
    pub selected_nodes: BTreeSet<NodeId>,
}

impl Default for GraphView {
    fn default() -> Self {
        Self {
            view_nodes: KeyIndexVec::default(),
            pan: Vec2::ZERO,
            scale: 1.0,
            selected_nodes: BTreeSet::new(),
        }
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

    /// Assign positions using topological-depth columns: nodes with no
    /// bound inputs go in column 0, downstream nodes shift right by one
    /// column per max-upstream-depth. Within a column, stack vertically
    /// in graph insertion order.
    pub fn auto_layout(
        &mut self,
        graph: &CoreGraph,
        col_spacing: f32,
        row_spacing: f32,
        origin: Vec2,
    ) {
        let mut depth: HashMap<NodeId, u32> = HashMap::new();
        for node in graph.iter() {
            let d = graph
                .edges()
                .filter(|(dst, _)| dst.node_id == node.id)
                .filter_map(|(_, src)| depth.get(&src.node_id).copied().map(|d| d + 1))
                .max()
                .unwrap_or(0);
            depth.insert(node.id, d);
        }

        let mut row_in_col: HashMap<u32, u32> = HashMap::new();
        for view_node in self.view_nodes.iter_mut() {
            let d = depth.get(&view_node.id).copied().unwrap_or(0);
            let row = row_in_col.entry(d).or_insert(0);
            view_node.pos = origin + Vec2::new(d as f32 * col_spacing, *row as f32 * row_spacing);
            *row += 1;
        }
    }

    /// `auto_layout` with the shared default column/row spacing + origin.
    pub fn auto_layout_default(&mut self, graph: &CoreGraph) {
        self.auto_layout(
            graph,
            AUTO_LAYOUT_COL_SPACING,
            AUTO_LAYOUT_ROW_SPACING,
            AUTO_LAYOUT_ORIGIN,
        );
    }

    fn validate(&self, graph: &CoreGraph) {
        assert!(
            self.scale.is_finite() && self.scale > 0.0,
            "graph zoom must be finite and positive"
        );
        assert!(
            self.pan.x.is_finite() && self.pan.y.is_finite(),
            "graph pan must be finite"
        );

        let mut view_nodes = HashMap::new();
        for node in self.view_nodes.iter() {
            assert!(
                node.pos.x.is_finite() && node.pos.y.is_finite(),
                "node position must be finite"
            );
            let prior = view_nodes.insert(node.id, ());
            assert!(prior.is_none(), "duplicate node id detected");
        }

        for selected in &self.selected_nodes {
            assert!(
                view_nodes.contains_key(selected),
                "selected node id must exist in graph"
            );
        }

        let mut graph_nodes = HashMap::new();
        for node in graph.iter() {
            let prior = graph_nodes.insert(node.id, ());
            assert!(prior.is_none(), "duplicate node id detected in graph");
        }

        assert_eq!(
            view_nodes.len(),
            graph_nodes.len(),
            "view node list must match graph nodes"
        );
        for node_id in graph_nodes.keys() {
            assert!(
                view_nodes.contains_key(node_id),
                "graph view missing node position"
            );
        }
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
    /// selection). Mirrors the old `Document::remove_node`.
    pub fn remove_node(&mut self, node_id: &NodeId) {
        self.view.view_nodes.retain(|node| node.id != *node_id);
        self.graph.remove_by_id(*node_id);
        self.view.selected_nodes.remove(node_id);
    }
}

/// The thing being edited: the core `Graph` (which already nests local
/// subgraph defs and their interior graphs) plus the editor view
/// metadata for each graph the user has open — the root in `main_view`,
/// every opened subgraph interior in `sub_views`. The `Library` it
/// resolves against lives one level up on `App` (runtime-owned).
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub graph: CoreGraph,
    pub main_view: GraphView,
    /// View metadata for local subgraph interiors, created lazily when a
    /// subgraph is first opened in a tab. Keyed by `SubgraphId`.
    #[serde(default)]
    pub sub_views: HashMap<SubgraphId, GraphView>,
    /// Open editor tabs, left to right. Always non-empty; `tabs[0]` is
    /// `TabRef::Graph(GraphRef::Main)`. Persisted + undoable like the rest
    /// of the view state (switching tabs is an undoable `Intent`).
    #[serde(default = "default_tabs")]
    pub tabs: Vec<TabRef>,
    /// Index into `tabs` of the visible tab.
    #[serde(default)]
    pub active: usize,
}

fn default_tabs() -> Vec<TabRef> {
    vec![TabRef::Graph(GraphRef::Main)]
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

impl Default for Document {
    fn default() -> Self {
        Self {
            graph: CoreGraph::default(),
            main_view: GraphView::default(),
            sub_views: HashMap::new(),
            tabs: default_tabs(),
            active: 0,
        }
    }
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

    /// The active tab (graph pane or a non-graph app view).
    pub fn active_tab(&self) -> TabRef {
        self.tabs[self.active]
    }

    /// The graph the active tab points at, or `None` when the active tab
    /// is a non-graph view (e.g. `Config`).
    pub fn active_target(&self) -> Option<GraphRef> {
        match self.tabs[self.active] {
            TabRef::Graph(target) => Some(target),
            TabRef::Config => None,
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
            view.auto_layout_default(&def.graph);
            view
        };
        self.sub_views.insert(id, view);
        true
    }

    /// Keep the tab list renderable: drop tabs whose graph vanished,
    /// seed any `Local` tab that's missing its view metadata, and clamp
    /// `active` into range — so the scene rebuild always resolves a live
    /// graph *and* view. `Main` always survives (`graph_for(Main)` is
    /// infallible and `main_view` always exists).
    ///
    /// The view-seeding covers a desync hazard: `tabs` and `sub_views`
    /// are independent serialized fields, so a deserialized (or
    /// hand-edited) document can carry a `Local` tab with no matching
    /// `sub_views` entry. Seeding it here recovers gracefully instead of
    /// panicking on a later `view(target).expect(..)`.
    pub fn ensure_valid_active(&mut self) {
        // Common case: every tab still resolves — touch nothing (no
        // per-frame allocation). Only rebuild the list when a tab's
        // graph actually vanished. Non-graph tabs (e.g. `Config`) always
        // resolve.
        if self.tabs.iter().any(|t| match t {
            TabRef::Graph(g) => self.graph_for(*g).is_none(),
            TabRef::Config => false,
        }) {
            // Split the borrow so `retain` (mut `tabs`) can read `graph`.
            let Document { graph, tabs, .. } = self;
            tabs.retain(|t| match t {
                TabRef::Graph(GraphRef::Main) | TabRef::Config => true,
                TabRef::Graph(GraphRef::Local(id)) => graph.subgraphs.by_key(id).is_some(),
            });
        }
        // Seed views for any `Local` tab missing one. Guarded by `any`
        // so the common (all-seeded) case allocates nothing.
        if self.tabs.iter().any(
            |t| matches!(t, TabRef::Graph(GraphRef::Local(id)) if !self.sub_views.contains_key(id)),
        ) {
            let missing: Vec<SubgraphId> = self
                .tabs
                .iter()
                .filter_map(|t| match t {
                    TabRef::Graph(GraphRef::Local(id)) if !self.sub_views.contains_key(id) => {
                        Some(*id)
                    }
                    _ => None,
                })
                .collect();
            for id in missing {
                self.ensure_sub_view(id);
            }
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
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
        self.graph.prune_bindings(library);
        self.graph.prune_subscriptions(library);
    }

    pub fn validate(&self) {
        if !is_debug() {
            return;
        }

        self.graph.validate();
        self.main_view.validate(&self.graph);

        // Each opened subgraph view must match its interior graph; a
        // view whose subgraph was deleted is a stale-entry bug.
        for (id, view) in &self.sub_views {
            let def = self
                .graph
                .subgraphs
                .by_key(id)
                .expect("sub_views entry references missing local subgraph");
            view.validate(&def.graph);
        }

        assert!(!self.tabs.is_empty(), "tab list must be non-empty");
        assert_eq!(
            self.tabs[0],
            TabRef::Graph(GraphRef::Main),
            "first tab must be Main"
        );
        assert!(self.active < self.tabs.len(), "active tab index in range");
        for tab in &self.tabs {
            if let TabRef::Graph(g) = tab {
                assert!(
                    self.graph_for(*g).is_some(),
                    "open tab references a missing graph"
                );
            }
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
        doc.validate();

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
            tabs: default_tabs(),
            active: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::prelude::FuncId;
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
        use crate::core::edit::intent::{Intent, apply_step, build_step, revert_step};

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
        assert!(doc.graph.by_id(&node_id).is_some(), "instance node added");

        revert_step(&step, &mut doc, GraphRef::Main);
        assert!(
            doc.graph.subgraphs.by_key(&local_id).is_none(),
            "undo removes the def"
        );
        assert!(
            doc.graph.by_id(&node_id).is_none(),
            "undo removes the instance node"
        );
    }

    /// Localize one library instance into `doc`'s root graph and return
    /// `(node_id, local_def_id)`. `origin` tags the copy's library
    /// lineage so a later instance can dedup against it.
    fn add_library_instance(doc: &mut Document, lib_id: SubgraphId) -> (NodeId, SubgraphId) {
        use crate::core::edit::intent::{Intent, apply_step, build_step};

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
            doc.graph.by_id(&node_b).unwrap().kind,
            NodeKind::Subgraph(SubgraphRef::Local(def_a_id)),
            "second instance points at the first instance's local def"
        );
    }

    #[test]
    fn detach_forks_standalone_copy_and_repoints_node() {
        use crate::core::edit::intent::{Intent, apply_step, build_step, revert_step};

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
        let NodeKind::Subgraph(SubgraphRef::Local(new_id)) =
            doc.graph.by_id(&node_id).unwrap().kind
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
        let NodeKind::Subgraph(SubgraphRef::Local(restored)) =
            doc.graph.by_id(&node_id).unwrap().kind
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
        use crate::core::edit::intent::{Intent, apply_step, build_step, revert_step};

        let mut doc = Document::default();
        let id = add_node_at(&mut doc, Vec2::ZERO);
        assert!(!doc.graph.by_id(&id).unwrap().disabled, "starts enabled");

        let step = build_step(
            Intent::SetDisabled {
                node_id: id,
                to: true,
            },
            &doc,
            GraphRef::Main,
        )
        .expect("builds");
        apply_step(&step, &mut doc, GraphRef::Main);
        assert!(doc.graph.by_id(&id).unwrap().disabled, "apply disables");

        revert_step(&step, &mut doc, GraphRef::Main);
        assert!(
            !doc.graph.by_id(&id).unwrap().disabled,
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

    #[test]
    fn config_tab_has_no_graph_target_and_survives_validation() {
        let mut doc = Document::default();
        // A graph tab resolves to a target; the appended Config tab does not.
        assert_eq!(doc.active_target(), Some(GraphRef::Main));
        doc.tabs.push(TabRef::Config);
        doc.active = 1;
        assert_eq!(doc.active_tab(), TabRef::Config);
        assert_eq!(doc.active_target(), None, "a non-graph tab has no target");

        // A non-graph tab always resolves, so it's never pruned and the
        // active index is left alone.
        doc.ensure_valid_active();
        assert_eq!(
            doc.tabs,
            vec![TabRef::Graph(GraphRef::Main), TabRef::Config]
        );
        assert_eq!(doc.active, 1);
        doc.validate();
    }

    #[test]
    fn ensure_valid_active_keeps_config_when_a_subgraph_tab_vanishes() {
        let mut doc = Document::default();
        let id = doc.create_subgraph();
        doc.tabs
            .extend([TabRef::Graph(GraphRef::Local(id)), TabRef::Config]);
        doc.active = 2; // viewing the config tab
        // Drop the subgraph out from under its open tab.
        doc.graph.subgraphs.remove_by_key(&id);

        doc.ensure_valid_active();
        // The dead subgraph tab is pruned; Main + Config remain, and active
        // is clamped back into range.
        assert_eq!(
            doc.tabs,
            vec![TabRef::Graph(GraphRef::Main), TabRef::Config]
        );
        assert_eq!(doc.active, 1);
    }

    #[test]
    fn config_tab_round_trips_in_every_format() {
        let mut doc: Document = core_test_graph().into();
        doc.tabs.push(TabRef::Config);
        for format in SerdeFormat::all_formats_for_testing() {
            let bytes = doc.serialize(format).expect("serialize with a config tab");
            let back = Document::deserialize(format, &bytes).expect("deserialize");
            assert_eq!(
                back.tabs, doc.tabs,
                "tabs (including Config) round-trip for {format:?}"
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
            doc.main_view.scale, deserialized.main_view.scale,
            "zoom should round-trip"
        );
        assert_eq!(
            doc.main_view.pan, deserialized.main_view.pan,
            "pan should round-trip"
        );
    }
}
