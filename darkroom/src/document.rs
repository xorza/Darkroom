use anyhow::{Result, bail};
use common::{SerdeFormat, is_debug, key_index_vec::KeyIndexVec};
use glam::Vec2;
use scenarium::graph::{Binding, InputPort, Node, NodeKind, OutputPort, Subscription};
use scenarium::prelude::{FuncLib, Graph as CoreGraph, NodeId, SubgraphDef, SubgraphId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

use crate::intent::Intent;
use crate::model::ViewNode;
use crate::reconcile::reconcile_def;

/// World-space offset applied to duplicated nodes so the copies don't
/// land exactly on top of their originals.
const DUPLICATE_OFFSET: Vec2 = Vec2::new(32.0, 32.0);

/// Initial placement of a fresh subgraph's boundary nodes: the input
/// boundary at the origin, the output boundary one gap to the right and
/// level with it (instead of the generic auto-layout stacking the two
/// unconnected nodes in one column).
const BOUNDARY_LAYOUT_ORIGIN: Vec2 = Vec2::new(40.0, 40.0);
const BOUNDARY_LAYOUT_GAP: f32 = 520.0;

/// Which graph an editor tab is pointed at. `Main` is the document's
/// root graph; `Local(id)` is a local subgraph def's interior graph
/// (`Document::graph.subgraphs[id].graph`). Linked subgraphs live in the
/// shared `FuncLib`, not the document, so they aren't editable targets
/// here yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GraphRef {
    Main,
    Local(SubgraphId),
}

/// Which side of a subgraph def's interface a boundary-port edit targets:
/// `Input` → `def.inputs` (the `SubgraphInput` node's output ports),
/// `Output` → `def.outputs` (the `SubgraphOutput` node's input ports).
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
/// every opened subgraph interior in `sub_views`. The `FuncLib` it
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
    /// `GraphRef::Main`. Persisted + undoable like the rest of the view
    /// state (switching tabs is an undoable `Intent`).
    #[serde(default = "default_tabs")]
    pub tabs: Vec<GraphRef>,
    /// Index into `tabs` of the visible tab.
    #[serde(default)]
    pub active: usize,
}

fn default_tabs() -> Vec<GraphRef> {
    vec![GraphRef::Main]
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

    /// The graph the active tab points at.
    pub fn active_target(&self) -> GraphRef {
        self.tabs[self.active]
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
        let def = SubgraphDef {
            id: SubgraphId::unique(),
            name: format!("subgraph {}", self.graph.subgraphs.len() + 1),
            category: String::new(),
            graph,
            inputs: Vec::new(),
            outputs: Vec::new(),
            events: Vec::new(),
        };
        let id = def.id;
        self.graph.subgraphs.add(def);

        // Seed the view explicitly so the pair opens input-left /
        // output-right; `ensure_sub_view` then finds it and skips the
        // generic auto-layout that would stack them.
        let mut view = GraphView::default();
        view.view_nodes.add(ViewNode {
            id: input_id,
            pos: BOUNDARY_LAYOUT_ORIGIN,
        });
        view.view_nodes.add(ViewNode {
            id: output_id,
            pos: BOUNDARY_LAYOUT_ORIGIN + Vec2::new(BOUNDARY_LAYOUT_GAP, 0.0),
        });
        self.sub_views.insert(id, view);

        id
    }

    /// Build a `DuplicateNodes` intent for `target`'s current selection:
    /// clone each selected node with a fresh id and an offset position,
    /// copy const-value bindings, and recreate the data + event
    /// connections whose *both* endpoints are selected (wires to
    /// unselected nodes are dropped). `None` when nothing is selected or
    /// the target doesn't resolve.
    pub fn duplicate_intent(&self, target: GraphRef) -> Option<Intent> {
        let EditScopeRef { graph, view } = self.scope(target)?;
        if view.selected_nodes.is_empty() {
            return None;
        }

        let mut id_map: HashMap<NodeId, NodeId> = HashMap::new();
        let mut nodes = Vec::new();
        for old_id in &view.selected_nodes {
            let Some(node) = graph.by_id(old_id) else {
                continue;
            };
            let new_id = NodeId::unique();
            id_map.insert(*old_id, new_id);
            let mut clone = node.clone();
            clone.id = new_id;
            let pos = view.view_nodes.by_key(old_id).unwrap().pos + DUPLICATE_OFFSET;
            nodes.push((ViewNode { id: new_id, pos }, clone));
        }

        // Each selected node's own input ports. Const/None copy verbatim;
        // a `Bind` survives only if its source is also selected (remapped
        // to the clone) — otherwise the wire is external and dropped.
        let mut bindings = Vec::new();
        for old_id in &view.selected_nodes {
            for (port, binding) in graph.bindings_touching(*old_id) {
                if port.node_id != *old_id {
                    continue;
                }
                let new_binding = match binding {
                    Binding::Bind(src) => match id_map.get(&src.node_id) {
                        Some(&new_src) => Binding::Bind(OutputPort {
                            node_id: new_src,
                            port_idx: src.port_idx,
                        }),
                        None => continue,
                    },
                    other => other,
                };
                bindings.push((InputPort::new(id_map[old_id], port.port_idx), new_binding));
            }
        }

        // Event subscriptions internal to the selection.
        let mut subscriptions = Vec::new();
        for s in graph.subscriptions() {
            if let (Some(&emitter), Some(&subscriber)) =
                (id_map.get(&s.emitter), id_map.get(&s.subscriber))
            {
                subscriptions.push(Subscription {
                    emitter,
                    event_idx: s.event_idx,
                    subscriber,
                });
            }
        }

        Some(Intent::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
        })
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
    /// scene rather than stored as undo steps. See `crate::reconcile` for
    /// the per-def logic and rationale (placeholder ports, compaction).
    pub fn reconcile_boundaries(&mut self, func_lib: &FuncLib) {
        if self.graph.subgraphs.is_empty() {
            return;
        }
        let def_ids: Vec<SubgraphId> = self.graph.subgraphs.iter().map(|d| d.id).collect();
        for id in def_ids {
            reconcile_def(self, id, func_lib);
        }
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
        assert_eq!(self.tabs[0], GraphRef::Main, "first tab must be Main");
        assert!(self.active < self.tabs.len(), "active tab index in range");
        for tab in &self.tabs {
            assert!(
                self.graph_for(*tab).is_some(),
                "open tab references a missing graph"
            );
        }
    }

    pub fn serialize(&self, format: SerdeFormat) -> Vec<u8> {
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
    use scenarium::prelude::{FuncId, StaticValue};
    use scenarium::subgraph::SubgraphRef;
    use scenarium::testing::test_graph as core_test_graph;

    /// A childless local def with the given id/name.
    fn leaf_def(id: SubgraphId, name: &str) -> SubgraphDef {
        SubgraphDef {
            id,
            name: name.into(),
            ..Default::default()
        }
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
        let parent = SubgraphDef {
            id: parent_id,
            name: "parent".into(),
            graph: interior,
            ..Default::default()
        };

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
    fn duplicate_intent_clones_internal_wiring_and_drops_external() {
        // a -> b (internal edge, both selected); c -> b (external, c not
        // selected). b also has a Const on input 1. Selecting {a, b} must
        // duplicate a' and b', keep a'->b' and the Const, drop c->b.
        let mut doc = Document::default();
        let a = add_node_at(&mut doc, Vec2::new(0.0, 0.0));
        let b = add_node_at(&mut doc, Vec2::new(100.0, 0.0));
        let c = add_node_at(&mut doc, Vec2::new(0.0, 100.0));
        doc.graph
            .set_input_binding(InputPort::new(b, 0), (a, 0).into());
        doc.graph.set_input_binding(
            InputPort::new(b, 1),
            Binding::Const(StaticValue::from(7i64)),
        );
        doc.graph
            .set_input_binding(InputPort::new(b, 2), (c, 0).into());
        doc.main_view.selected_nodes = [a, b].into_iter().collect();

        let Some(Intent::DuplicateNodes {
            nodes,
            bindings,
            subscriptions,
        }) = doc.duplicate_intent(GraphRef::Main)
        else {
            panic!("expected a DuplicateNodes intent");
        };

        assert_eq!(nodes.len(), 2, "both selected nodes cloned");
        assert!(subscriptions.is_empty());
        // Fresh ids, offset positions.
        let new_ids: BTreeSet<NodeId> = nodes.iter().map(|(_, n)| n.id).collect();
        assert!(
            new_ids.is_disjoint(&doc.main_view.selected_nodes),
            "clones get fresh ids"
        );
        let a_clone = nodes
            .iter()
            .find(|(vn, _)| vn.pos == Vec2::new(0.0, 0.0) + DUPLICATE_OFFSET)
            .map(|(_, n)| n.id)
            .expect("a's clone offset from its origin");

        // Exactly two bindings survive: the internal a'->b' edge and the
        // Const; the external c->b edge (input 2) is gone.
        assert_eq!(bindings.len(), 2);
        let b_clone = nodes
            .iter()
            .find(|(vn, _)| vn.pos == Vec2::new(100.0, 0.0) + DUPLICATE_OFFSET)
            .map(|(_, n)| n.id)
            .unwrap();
        let internal = bindings
            .iter()
            .find(|(port, _)| port.port_idx == 0)
            .expect("a'->b' edge present");
        assert_eq!(internal.0.node_id, b_clone, "edge sinks into b's clone");
        match &internal.1 {
            Binding::Bind(src) => {
                assert_eq!(src.node_id, a_clone, "remapped to a's clone");
                assert_eq!(src.port_idx, 0);
            }
            other => panic!("expected Bind, got {other:?}"),
        }
        assert!(
            bindings
                .iter()
                .any(|(port, bind)| port.port_idx == 1 && matches!(bind, Binding::Const(_))),
            "const binding copied"
        );
        assert!(
            !bindings.iter().any(|(port, _)| port.port_idx == 2),
            "external edge dropped"
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

        // Each create mints a distinct id (no overwrite).
        let id2 = doc.create_subgraph();
        assert_ne!(id, id2);
        assert_eq!(doc.graph.subgraphs.len(), 2);
    }

    #[test]
    fn duplicate_intent_none_without_selection() {
        let mut doc = Document::default();
        add_node_at(&mut doc, Vec2::ZERO);
        assert!(doc.duplicate_intent(GraphRef::Main).is_none());
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
        let serialized = doc.serialize(format);
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
