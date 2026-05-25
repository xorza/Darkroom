use anyhow::{Result, bail};
use common::{SerdeFormat, is_debug, key_index_vec::KeyIndexVec};
use glam::Vec2;
use scenarium::prelude::{FuncLib, Graph as CoreGraph, NodeId, SubgraphId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

use crate::model::ViewNode;
use crate::reconcile::reconcile_def;

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
    use scenarium::testing::test_graph as core_test_graph;

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
