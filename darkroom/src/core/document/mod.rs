//! The document being edited, and how a decoded one is made consistent.
//!
//! # Decisions
//!
//! - **A stub is scenarium's.** A node its graph cannot hold is kept in the graph beside its
//!   nodes as a [`scenarium::Stub`], with every edge that touched it and why, so your
//!   settings survive a library that lost its func and a definition travels whole; the load is
//!   [`GraphSet::reconcile`]. Refused: dropping them, as the settings are lost; remapping by port
//!   name, as a value whose meaning changed still goes through.
//! - **A graph with a stub runs the rest.** A consumer a stub fed sees an
//!   unbound input, as with a disabled producer. Refused: one stub refusing
//!   every run.
//! - **A stub can be selected, moved and deleted, not edited or copied.** It
//!   has no ports and a read-only title, and Duplicate skips it: a copy of a
//!   node that cannot run has little use.
//! - **View and layout state is repaired, not refused.** Refused: refusing the
//!   file, as a bad viewport alone would keep the graph closed.
//! - **A removal closes what stops resolving.** Every removal — an edit, an undo,
//!   a redo — goes through [`Document::remove_node`], which closes each tab and clears each
//!   `through` the removal broke; an undo of the removal does not reopen the tab. Refused:
//!   scanning every tab each frame.
//! - **The editor keeps a side beside each subgraph**: its view and the instance its pane shows
//!   it through, keyed as the set's subgraphs are. Refused: the view inside scenarium's
//!   `Subgraph`, which puts the editor's layout in the framework; a `GraphSet` generic over what
//!   a host keeps beside a subgraph, which threads a type parameter through the scope and the
//!   compile.
//! - **The window has one active graph**, the graph of the pane that last took input: the menu
//!   bar, the status bar, the selection's inspector and each shortcut act on it, and an edit that
//!   removes it hands that to the main graph. A picked tab is input on its pane, so the menus act
//!   on the graph whose tab was just picked. As VS Code's active editor, the one that "currently
//!   has focus"
//!   ([`window.activeTextEditor`](https://code.visualstudio.com/api/references/vscode-api)).
//!   Refused: the main graph's selection as the one selection; the active graph moved only by an
//!   edit, which leaves the menus on a pane no longer in front.
//! - **One pane per graph, shown through one instance.** The main graph has its tab and each
//!   subgraph at most one, whose `through` names the instance whose statuses, previews and timings
//!   it shows; opened from the palette it shows none. As Blender's Tab and ComfyUI's navigation
//!   bar.
//! - **Entering is navigation.** A double click on an instance, a name in the pane header and the
//!   status bar's link to a failure inside raise one `Enter`, which sets the side's `through`,
//!   opens or focuses the tab and makes the graph active, recording nothing. Refused: an undoable
//!   step, as the camera and the tabs stay out of the history.
//! - **An undo brings the graph it changes to the front** when its tab is open, and the status
//!   bar names it when not, so no undo changes only what is off screen.
//! - **An unused subgraph stays until removed**: the palette marks it, and the Graph menu's
//!   Remove Unused clears them, outermost first, in one entry. Refused: removal with the last
//!   instance, which loses work in Blender unless a fake user is set. Cost: a collapse and an
//!   expand leave one behind.

pub(crate) mod error;
pub(crate) mod graph_revision;
pub(crate) mod graph_view;
pub(crate) mod inspect_target;
pub(crate) mod item_id;
mod layout_file;
pub(crate) mod load_report;
pub(crate) mod route;
pub(crate) mod subgraph_side;
pub(crate) mod tab_ref;
pub(crate) mod tie;
pub(crate) mod viewport;
pub(crate) mod wire_key;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::slice;

use glam::Vec2;
use palantir::{DockOperation, DockState, TabGroup};
use scenarium::{
    ConstValue, DetachedNode, Graph as CoreGraph, GraphId, GraphSet, InstancePath, Library, Node,
    NodeAddr, NodeId, PathForm, ProgramId, Stub, Subgraph, SubgraphId,
};
use serde::{Deserialize, Serialize};

use crate::core::document::error::DocumentValidationError;
use crate::core::document::graph_view::{GraphView, NodePlacement};
use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::item_id::ItemId;
use crate::core::document::load_report::{DocumentRepair, ForeignPath, LoadReport, ViewRepair};
use crate::core::document::route::Route;
use crate::core::document::subgraph_side::SubgraphSide;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::tie::{TieAngle, TieId};
use crate::core::document::viewport::Viewport;
use crate::core::edit::edit_facts::Touched;
use crate::core::preview_sink;

/// The thing being edited: its graphs, each with the stubs it keeps, the editor's view of each,
/// and the pane layout showing them. The `Library` they resolve against lives one level up on
/// `App`.
///
/// A decoded document is made consistent once, by [`Self::reconcile`] at
/// load; every edit after keeps it so, and nothing after re-checks it.
///
/// The fields are public because a caller that touches several wants them borrowed together, and
/// destructuring proves they are disjoint, where accessor methods would each borrow the whole
/// `Document`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Document {
    /// Each graph holds only nodes its scope resolves, with only admitted wiring, and keeps each
    /// node it cannot hold as a stub.
    pub(crate) graphs: GraphSet,
    /// Places every node and every stub of the main graph.
    pub(crate) main_view: GraphView,
    /// One per subgraph of `graphs`, as a view holds one placement per node and stub.
    pub(crate) sides: BTreeMap<SubgraphId, SubgraphSide>,
    /// The pane arrangement: open tabs grouped into split panes, plus the focused group.
    /// Mutated only through a `DockOperation`, none of which is undoable.
    #[serde(default = "new_layout", with = "layout_file")]
    pub(crate) layout: DockState<TabRef>,
    /// The graph the window's commands act on: the selection a menu or the status bar reads, and
    /// the graph a Duplicate or a Delete edits. View state, not undoable; a request against a
    /// graph makes it the active one.
    pub(crate) active: GraphId,
}

/// One graph of the document and its view, borrowed apart: what an edit of one graph writes.
#[derive(Debug)]
pub(crate) struct GraphSlot<'a> {
    pub(crate) graph: &'a mut CoreGraph,
    pub(crate) view: &'a mut GraphView,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            graphs: GraphSet::default(),
            main_view: GraphView::default(),
            sides: BTreeMap::new(),
            layout: new_layout(),
            active: GraphId::Main,
        }
    }
}

impl Document {
    /// The view of `graph`: its placements, ties, camera and selection.
    ///
    /// # Panics
    /// If the document holds no `graph`.
    pub(crate) fn view(&self, graph: GraphId) -> &GraphView {
        match graph {
            GraphId::Main => &self.main_view,
            GraphId::Subgraph(id) => &self.sides.get(&id).expect("each subgraph has a side").view,
        }
    }

    /// `graph` and its view, to write: what an edit of one graph takes.
    ///
    /// # Panics
    /// As [`Self::view`].
    pub(crate) fn slot_mut(&mut self, graph: GraphId) -> GraphSlot<'_> {
        let view = match graph {
            GraphId::Main => &mut self.main_view,
            GraphId::Subgraph(id) => {
                &mut self
                    .sides
                    .get_mut(&id)
                    .expect("each subgraph has a side")
                    .view
            }
        };
        GraphSlot {
            graph: self.graphs.graph_mut(graph),
            view,
        }
    }

    /// The instance `graph`'s pane shows its run results through: the main graph's own path, a
    /// subgraph's side's, or `None` for a subgraph shown through none.
    ///
    /// # Panics
    /// If the document holds no `graph`.
    pub(crate) fn through(&self, graph: GraphId) -> Option<&InstancePath> {
        match graph {
            GraphId::Main => Some(&InstancePath::MAIN),
            GraphId::Subgraph(id) => self
                .sides
                .get(&id)
                .expect("each subgraph has a side")
                .through
                .as_ref(),
        }
    }

    /// Apply a change of the pane arrangement, then make the graph the focused pane shows the
    /// active one: a tab picked or a pane focused is input on that pane, as VS Code's active
    /// editor is the one that "currently has focus"
    /// ([`window.activeTextEditor`](https://code.visualstudio.com/api/references/vscode-api)).
    /// A focused pane that shows no graph leaves the active graph as it was.
    pub(crate) fn apply_view(&mut self, op: DockOperation<TabRef>) {
        self.layout.apply(op);
        let focused = self.layout.focused();
        if let Some(TabRef::Graph(graph)) = self.layout.group(focused).map(TabGroup::active_tab) {
            self.active = graph;
        }
    }

    /// Show `subgraph`'s pane through `through`, open or focus its tab, and make it the active
    /// graph: the entry into a subgraph. Navigation, as a tab switch is: it records nothing. A
    /// subgraph the set no longer holds, or a path that names no instance of it, raised against
    /// an earlier frame's document, enters nothing.
    pub(crate) fn enter(&mut self, subgraph: SubgraphId, through: Option<InstancePath>) {
        let graph = GraphId::Subgraph(subgraph);
        let Some(side) = self.sides.get_mut(&subgraph) else {
            return;
        };
        if through.is_some_and(|path| !shows(&self.graphs, path, graph)) {
            return;
        }
        side.through = through;
        self.layout.apply(DockOperation::OpenTab {
            tab: TabRef::Graph(graph),
        });
        self.active = graph;
    }

    /// Put `subgraph` into the document under `id`, with `side` beside it.
    ///
    /// # Panics
    /// If the document holds a subgraph under `id`.
    pub(crate) fn insert_subgraph(
        &mut self,
        id: SubgraphId,
        subgraph: Subgraph,
        side: SubgraphSide,
    ) {
        self.graphs.insert_subgraph(id, subgraph);
        let prev = self.sides.insert(id, side);
        assert!(prev.is_none(), "a subgraph id names one side");
    }

    /// Take subgraph `id` and its side out of the document, then close each tab and clear each
    /// `through` that stopped resolving, and hand the active graph to the main graph when it was
    /// `id`.
    ///
    /// # Panics
    /// If the document holds no subgraph `id`, or a node places it.
    pub(crate) fn remove_subgraph(&mut self, id: SubgraphId) -> (Subgraph, SubgraphSide) {
        let subgraph = self
            .graphs
            .remove_subgraph(id)
            .expect("a removed subgraph is in the set");
        let side = self.sides.remove(&id).expect("each subgraph has a side");
        if self.active == GraphId::Subgraph(id) {
            self.active = GraphId::Main;
        }
        self.prune_views(&mut |_| {});
        (subgraph, side)
    }

    /// Put `subgraph` and its `view` in place of the definition held under `id`, its interface
    /// the same, then close each tab and clear each `through` that stopped resolving; the
    /// definition and view it replaces.
    ///
    /// # Panics
    /// As [`GraphSet::replace_subgraph`].
    pub(crate) fn replace_subgraph(
        &mut self,
        id: SubgraphId,
        subgraph: Subgraph,
        view: GraphView,
    ) -> (Subgraph, GraphView) {
        let replaced = self.graphs.replace_subgraph(id, subgraph);
        let side = self.sides.get_mut(&id).expect("each subgraph has a side");
        let view = mem::replace(&mut side.view, view);
        self.prune_views(&mut |_| {});
        (replaced, view)
    }

    /// `base`, or `base N` with the least `N` from 2 that no subgraph of the document is named:
    /// the name a new definition takes.
    pub(crate) fn free_subgraph_name(&self, base: &str) -> String {
        let taken = |name: &str| {
            self.graphs
                .subgraphs()
                .any(|(_, subgraph)| subgraph.name == name)
        };
        if !taken(base) {
            return base.to_string();
        }
        // One name is free past the count of subgraphs.
        let last = self.graphs.subgraphs().len() + 2;
        (2..=last)
            .map(|n| format!("{base} {n}"))
            .find(|name| !taken(name))
            .expect("a name past the count of subgraphs is free")
    }

    /// The subgraphs no node places, and then those only they placed, in the order their
    /// removals hold: each placed by none of the subgraphs that remain. What Remove Unused takes
    /// out; empty when every subgraph is in use.
    pub(crate) fn unused_subgraphs(&self) -> Vec<SubgraphId> {
        let mut unused: Vec<SubgraphId> = Vec::new();
        // Each round finds at least one, or ends: bounded by the count of subgraphs.
        loop {
            let found: Vec<SubgraphId> = self
                .graphs
                .subgraphs()
                .map(|(id, _)| id)
                .filter(|id| !unused.contains(id))
                .filter(|&id| {
                    self.graphs.uses(id).all(|addr| match addr.graph {
                        GraphId::Main => false,
                        GraphId::Subgraph(holder) => unused.contains(&holder),
                    })
                })
                .collect();
            if found.is_empty() {
                return unused;
            }
            unused.extend(found);
        }
    }

    /// Bring the graph an undo or a redo wrote to the front: its tab activated and the graph made
    /// active, when it has a tab. The graph left with no tab to show it, for the status bar to
    /// name; `None` when the edit wrote no one graph, or one on screen now.
    pub(crate) fn bring_forward(&mut self, touched: Touched) -> Option<GraphId> {
        let Touched::Graph(graph) = touched else {
            return None;
        };
        let tab = TabRef::Graph(graph);
        if !self.layout.all_tabs().any(|open| open == tab) {
            return Some(graph);
        }
        self.layout.apply(DockOperation::ActivateTab { tab });
        self.active = graph;
        None
    }

    /// Drop a node of `graph` from the document: the graph, its placement and its selection
    /// membership, then each tab and each `through` the removal stopped resolving. Every removal
    /// (an edit, an undo or a redo) comes through here, so no tab outlives what it shows.
    pub(crate) fn remove_node(&mut self, graph: GraphId, node_id: NodeId) -> DetachedNode {
        let slot = self.slot_mut(graph);
        unplace(slot.view, node_id);
        let record = slot.graph.detach_node(node_id);
        self.prune_views(&mut |_| {});
        record
    }

    /// [`Self::remove_node`] for a stub. A stub has no viewer and places nothing, so nothing
    /// else stops resolving with it.
    pub(crate) fn remove_stub(&mut self, graph: GraphId, node_id: NodeId) -> Stub {
        let slot = self.slot_mut(graph);
        unplace(slot.view, node_id);
        slot.graph
            .remove_stub(node_id)
            .expect("cannot remove a stub the document does not hold")
    }

    /// Clear each side's `through` that stopped naming an instance of its subgraph, then close
    /// each tab the document stopped holding, handing `pruned` each change.
    fn prune_views(&mut self, pruned: &mut impl FnMut(DocumentRepair)) {
        let Self { graphs, sides, .. } = self;
        for (&subgraph, side) in sides.iter_mut() {
            if let Some(path) = side.through
                && !shows(graphs, path, GraphId::Subgraph(subgraph))
            {
                side.through = None;
                pruned(DocumentRepair::ThroughCleared { subgraph, path });
            }
        }
        let stale: Vec<TabRef> = self
            .layout
            .all_tabs()
            .filter(|&tab| !self.holds_tab(tab))
            .collect();
        // `retain_tabs` rebuilds the whole node tree, so it runs only when there is a tab to
        // close.
        if !stale.is_empty() {
            self.layout.retain_tabs(|tab| !stale.contains(&tab));
        }
        for tab in stale {
            pruned(DocumentRepair::TabClosed { tab });
        }
    }

    /// Whether a tab may be open: `Preferences` and the selection's inspector always, a graph's
    /// pane while the set holds the graph, a viewer exactly while its path names a preview node,
    /// and a pinned inspector while its graph holds its node and its `through` names an instance
    /// of that graph.
    pub(crate) fn holds_tab(&self, tab: TabRef) -> bool {
        match tab {
            TabRef::Preferences | TabRef::Inspector(InspectTarget::Selection) => true,
            TabRef::Graph(graph) => holds_graph(&self.graphs, graph),
            TabRef::ImageViewer(program_id) => self.holds_preview_node(program_id),
            TabRef::Inspector(InspectTarget::Node { addr, through }) => {
                holds_graph(&self.graphs, addr.graph)
                    && self.graphs.graph(addr.graph).find(addr.node_id).is_some()
                    && through.is_none_or(|path| shows(&self.graphs, path, addr.graph))
            }
        }
    }

    /// Whether `program_id` names a preview node the document holds: its path ends in a graph of
    /// the set, which holds its node as a preview node. The retention of the value one published.
    /// Stricter than a graph's [`Graph::holds`](scenarium::Graph::holds): the preview store holds
    /// textures up to 8192² RGBA8, so a value whose key is not a live preview node releases at the
    /// next sweep instead of riding on the node's own lifetime.
    pub(crate) fn holds_preview_node(&self, program_id: ProgramId) -> bool {
        self.graphs
            .graph_at(program_id.path())
            .is_some_and(|graph| is_preview_node(self.graphs.graph(graph), program_id.node_id()))
    }

    /// Make a decoded document consistent with itself and `library`, and
    /// report each change: [`GraphSet::reconcile`] the graphs, which keeps
    /// each node a graph cannot hold as a stub and attaches each stub that
    /// resolves again; then give each subgraph a side and drop each side of
    /// none, repair each view against its graph, clear each `through` that
    /// names no instance of its subgraph, make the main graph active in place
    /// of a graph the set lacks, and close each tab the document no longer
    /// holds. Afterwards [`Self::validate`] holds, and the graphs compile.
    pub(crate) fn reconcile(&mut self, library: &Library) -> LoadReport {
        let reconciled = self.graphs.reconcile(library);
        let mut repairs: Vec<DocumentRepair> = reconciled
            .main
            .repairs
            .into_iter()
            .map(|repair| DocumentRepair::Graph {
                graph: GraphId::Main,
                repair,
            })
            .collect();
        for (id, reconciled) in reconciled.subgraphs {
            repairs.extend(
                reconciled
                    .repairs
                    .into_iter()
                    .map(|repair| DocumentRepair::Graph {
                        graph: GraphId::Subgraph(id),
                        repair,
                    }),
            );
        }
        let Self {
            graphs,
            main_view,
            sides,
            active,
            ..
        } = self;
        sides.retain(|&subgraph, _| {
            let held = graphs.subgraph(subgraph).is_some();
            if !held {
                repairs.push(DocumentRepair::SideDropped { subgraph });
            }
            held
        });
        for (subgraph, held) in graphs.subgraphs() {
            if let Entry::Vacant(side) = sides.entry(subgraph) {
                side.insert(SubgraphSide::new(&held.graph));
                repairs.push(DocumentRepair::SideAdded { subgraph });
            }
        }
        repair_view(GraphId::Main, main_view, graphs.main(), &mut repairs);
        for (&subgraph, side) in sides.iter_mut() {
            let graph = GraphId::Subgraph(subgraph);
            repair_view(graph, &mut side.view, graphs.graph(graph), &mut repairs);
        }
        if !holds_graph(graphs, *active) {
            repairs.push(DocumentRepair::ActiveReset { graph: *active });
            *active = GraphId::Main;
        }
        self.prune_views(&mut |repair| repairs.push(repair));
        debug_assert!(
            self.validate().is_ok(),
            "a reconciled document is consistent: {:?}",
            self.validate()
        );
        let mut foreign_paths = Vec::new();
        self.for_each_path(|node, path| {
            if PathForm::of(path) == PathForm::Foreign {
                foreign_paths.push(ForeignPath {
                    node,
                    path: path.to_string(),
                });
            }
        });
        LoadReport {
            repairs,
            foreign_paths,
            unknown_types: self.graphs.unknown_types(library),
            refused_entries: Vec::new(),
        }
    }

    /// Check what [`Self::reconcile`] establishes that needs no library: each graph's structure
    /// with its stubs, a side for each subgraph and for nothing else, each view against its
    /// graph, each `through` against the set, the active graph, and the layout. A debug check: a
    /// document that fails it is a bug in the load or an edit.
    pub(crate) fn validate(&self) -> Result<(), DocumentValidationError> {
        for (graph, held) in self.graphs.graphs() {
            held.validate()
                .map_err(|source| DocumentValidationError::Graph { graph, source })?;
        }
        if let Some(subgraph) = self
            .graphs
            .subgraphs()
            .map(|(id, _)| id)
            .find(|id| !self.sides.contains_key(id))
            .or_else(|| {
                self.sides
                    .keys()
                    .copied()
                    .find(|&id| self.graphs.subgraph(id).is_none())
            })
        {
            return Err(DocumentValidationError::SideMismatch { subgraph });
        }
        for (graph, held) in self.graphs.graphs() {
            self.view(graph)
                .validate(held)
                .map_err(|source| DocumentValidationError::View { graph, source })?;
        }
        for (&subgraph, side) in &self.sides {
            if let Some(path) = side.through
                && !shows(&self.graphs, path, GraphId::Subgraph(subgraph))
            {
                return Err(DocumentValidationError::StaleThrough { subgraph, path });
            }
        }
        if !holds_graph(&self.graphs, self.active) {
            return Err(DocumentValidationError::ActiveMissing { graph: self.active });
        }
        for tab in self.layout.all_tabs() {
            if !self.holds_tab(tab) {
                return Err(DocumentValidationError::MissingTab { tab });
            }
        }
        // A file holds relative paths; memory resolves each where it enters, so a relative one
        // here would be read against the working directory.
        let mut relative = None;
        self.for_each_path(|node, path| {
            if relative.is_none() && PathForm::of(path) == PathForm::Relative {
                relative = Some(DocumentValidationError::RelativePath {
                    node,
                    path: path.to_string(),
                });
            }
        });
        relative.map_or(Ok(()), Err)
    }

    /// Each path a constant holds in any graph, the stubs' included, with the node it binds.
    pub(crate) fn for_each_path(&self, mut visit: impl FnMut(NodeAddr, &str)) {
        for (graph, held) in self.graphs.graphs() {
            for (port, value) in held.constants() {
                for path in value.as_fs_paths().into_iter().flatten() {
                    visit(NodeAddr::new(graph, port.node_id), path);
                }
            }
        }
    }

    /// Put `map`'s answer in place of each path a constant holds in any graph that it answers
    /// for, the stubs' included.
    pub(crate) fn map_paths(&mut self, mut map: impl FnMut(&str) -> Option<String>) {
        let ids: Vec<GraphId> = self.graphs.graphs().map(|(graph, _)| graph).collect();
        for graph in ids {
            for value in self.graphs.graph_mut(graph).constants_mut() {
                let paths: &mut [String] = match value {
                    ConstValue::FsPath(path) => slice::from_mut(path),
                    ConstValue::FsPaths(paths) => paths,
                    _ => continue,
                };
                for path in paths {
                    if let Some(mapped) = map(path) {
                        *path = mapped;
                    }
                }
            }
        }
    }
}

/// Drop `node_id`'s placement and selection membership from `view`.
fn unplace(view: &mut GraphView, node_id: NodeId) {
    view.node_placements.remove(&node_id);
    view.selected.remove(&ItemId::Node(node_id));
}

impl From<GraphSet> for Document {
    /// A document of `graphs`, each graph's nodes placed in a row.
    fn from(graphs: GraphSet) -> Self {
        let main_view = GraphView::new(graphs.main());
        let sides = graphs
            .subgraphs()
            .map(|(id, subgraph)| (id, SubgraphSide::new(&subgraph.graph)))
            .collect();
        Self {
            graphs,
            main_view,
            sides,
            layout: new_layout(),
            active: GraphId::Main,
        }
    }
}

/// Whether `graphs` holds `graph`: the main graph, or a subgraph it holds.
fn holds_graph(graphs: &GraphSet, graph: GraphId) -> bool {
    match graph {
        GraphId::Main => true,
        GraphId::Subgraph(id) => graphs.subgraph(id).is_some(),
    }
}

/// Whether `path` names an instance whose expansion is `graph`: the main graph's own path, or a
/// path of instances that ends in `graph`.
fn shows(graphs: &GraphSet, path: InstancePath, graph: GraphId) -> bool {
    graphs.graph_at(path) == Some(graph)
}

/// Reset a bad viewport, drop each placement for no node or stub, move each
/// non-finite position to the origin, place each node or stub that has no
/// placement at the origin, repair the ties, and drop each selection entry
/// the view does not hold: `view` of `graph`, which `held` is.
fn repair_view(
    graph: GraphId,
    view: &mut GraphView,
    held: &CoreGraph,
    repairs: &mut Vec<DocumentRepair>,
) {
    let mut push = |repair| repairs.push(DocumentRepair::View { graph, repair });
    if !view.viewport.is_valid() {
        view.viewport = Viewport::default();
        push(ViewRepair::ViewportReset);
    }
    let placements = &mut view.node_placements;
    placements.retain(|&node_id, _| {
        let kept = held.holds(node_id);
        if !kept {
            push(ViewRepair::PlacementDropped { node_id });
        }
        kept
    });
    for (&node_id, placement) in placements.iter_mut() {
        if !placement.pos.is_finite() {
            placement.pos = Vec2::ZERO;
            push(ViewRepair::PosReset { node_id });
        }
    }
    let mut unplaced: Vec<NodeId> = held
        .iter()
        .map(|node| node.node_id)
        .chain(held.stubs().map(Stub::node_id))
        .filter(|node_id| !placements.contains_key(node_id))
        .collect();
    unplaced.sort_unstable();
    for node_id in unplaced {
        placements.insert(node_id, NodePlacement::default());
        push(ViewRepair::Placed { node_id });
    }
    repair_ties(view, held, &mut push);
    let mut selected = mem::take(&mut view.selected);
    selected.retain(|&item| {
        let kept = view.holds(item);
        if !kept {
            push(ViewRepair::SelectionDropped { item });
        }
        kept
    });
    view.selected = selected;
}

/// Drop each route to a wire the graph does not hold, each step to no tie
/// and each repeat of a step, and each route left with no step; then drop
/// each tie no route names, move each non-finite tie to the origin, and free
/// each pinned angle outside `[0, 360)`.
fn repair_ties(view: &mut GraphView, held: &CoreGraph, push: &mut impl FnMut(ViewRepair)) {
    let GraphView { ties, routes, .. } = view;
    routes.retain(|&wire, route| {
        if !wire.is_held_by(held) {
            push(ViewRepair::RouteDropped { wire });
            return false;
        }
        let mut steps: Vec<TieId> = Vec::with_capacity(route.iter().len());
        for &tie in route.iter() {
            if !ties.contains_key(&tie) {
                push(ViewRepair::RouteStepDropped { wire, tie });
            } else if steps.contains(&tie) {
                push(ViewRepair::RouteStepRepeated { wire, tie });
            } else {
                steps.push(tie);
            }
        }
        if steps.is_empty() {
            push(ViewRepair::EmptyRouteDropped { wire });
            return false;
        }
        if steps.len() != route.iter().len() {
            *route = Route::new(steps);
        }
        true
    });
    let routed: BTreeSet<TieId> = routes.values().flatten().copied().collect();
    ties.retain(|&tie, placed| {
        if !routed.contains(&tie) {
            push(ViewRepair::TieDropped { tie });
            return false;
        }
        if !placed.pos.is_finite() {
            placed.pos = Vec2::ZERO;
            push(ViewRepair::TiePosReset { tie });
        }
        if !placed.angle.is_valid() {
            placed.angle = TieAngle::Auto;
            push(ViewRepair::TieAngleFreed { tie });
        }
        true
    });
}

/// Whether `graph` holds `node_id` as a preview node.
fn is_preview_node(graph: &CoreGraph, node_id: NodeId) -> bool {
    graph
        .find(node_id)
        .and_then(Node::func_id)
        .is_some_and(preview_sink::is_func)
}

/// A fresh dock: one pane showing the main graph, the pinned tab, so the tree is never empty.
fn new_layout() -> DockState<TabRef> {
    DockState::new(TabRef::Graph(GraphId::Main))
}

#[cfg(feature = "internals")]
pub(crate) mod internals;

#[cfg(test)]
mod tests;
