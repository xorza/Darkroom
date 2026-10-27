//! The document being edited, and how a decoded one is made consistent.
//!
//! # Decisions
//!
//! - **Stubs are darkroom's, not scenarium's.** A node the library cannot
//!   resolve is kept beside the graph as the record a detach leaves — the node
//!   with every edge that touched it — so your settings survive a library that
//!   lost its func. Refused: stubs as graph nodes, as every reader then needs
//!   a "declares nothing" branch; dropping them, as the settings are lost;
//!   remapping by port name, as a value whose meaning changed still goes
//!   through.
//! - **A graph with a stub runs the rest.** A consumer a stub fed sees an
//!   unbound input, as with a disabled producer. Refused: one stub refusing
//!   every run.
//! - **A stub can be selected, moved and deleted, not edited or copied.** It
//!   has no ports and a read-only title, and Duplicate skips it: a copy of a
//!   node that cannot run has little use.
//! - **A stub's record is a frozen snapshot.** When its func resolves at a
//!   later load it is attached again; an edge whose other end is gone, or whose
//!   input the graph bound since, is dropped and reported. An edge between two
//!   stubs belongs to the record detached first.
//! - **View and layout state is repaired, not refused.** Refused: refusing the
//!   file, as a bad viewport alone would keep the graph closed.
//! - **A removal closes its node's viewer.** Every removal — an edit, an undo,
//!   a redo — goes through [`Document::remove_node`]; an undo of the removal
//!   does not reopen the tab. Refused: scanning every tab each frame.

pub(crate) mod error;
pub(crate) mod graph_revision;
pub(crate) mod graph_view;
pub(crate) mod inspect_target;
pub(crate) mod item_id;
pub(crate) mod load_report;
pub(crate) mod route;
pub(crate) mod stubs;
pub(crate) mod tab_ref;
pub(crate) mod tie;
pub(crate) mod viewport;
pub(crate) mod wire_key;

use std::collections::BTreeSet;
use std::path::Path;
use std::slice;

use glam::Vec2;
use palantir::DockState;
use scenarium::{
    Binding, ConstValue, DetachedNode, DetachedNodeParts, Detachment, Graph as CoreGraph, Library,
    Node, NodeId,
};
use serde::{Deserialize, Serialize};

use crate::core::document::error::DocumentValidationError;
use crate::core::document::graph_view::{GraphView, NodePlacement};
use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::item_id::ItemId;
use crate::core::document::load_report::{DocumentRepair, LoadReport};
use crate::core::document::route::Route;
use crate::core::document::stubs::Stubs;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::tie::{TieAngle, TieId};
use crate::core::document::viewport::Viewport;
use crate::core::preview_sink;

/// The thing being edited: the authoring `Graph`, the stubs kept beside it,
/// the editor view metadata positioning both, and the pane layout showing
/// them. The `Library` they resolve against lives one level up on `App`
/// (runtime-owned).
///
/// A decoded document is made consistent once, by [`Self::reconcile`] at
/// load; every edit after keeps it so, and nothing after re-checks it.
///
/// The fields are public because a caller that touches several wants them
/// borrowed together, and destructuring is what proves they are disjoint —
/// accessor methods would each borrow the whole `Document` and couldn't be
/// held at once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Document {
    /// Holds only nodes the library resolves, with only admitted wiring.
    pub(crate) graph: CoreGraph,
    /// The nodes the library cannot resolve. Their ids are disjoint from the
    /// graph's.
    #[serde(default)]
    pub(crate) stubs: Stubs,
    /// Places every graph node and every stub.
    pub(crate) main_view: GraphView,
    /// The pane arrangement: open tabs grouped into split panes, plus
    /// the focused group. Persisted like the rest of the view state, and
    /// mutated only through a `DockOperation` — none of which is undoable, so
    /// Ctrl+Z walks past a tab switch to the last graph edit.
    #[serde(default = "new_layout")]
    pub(crate) layout: DockState<TabRef>,
}

/// Scopes every widget id the dock derives. One string, in one place,
/// because two docks built from different seeds would draw with
/// different ids and share none of their state.
const DOCK_SEED: &str = "darkroom.dock";

impl Default for Document {
    fn default() -> Self {
        Self {
            graph: CoreGraph::default(),
            stubs: Stubs::default(),
            main_view: GraphView::default(),
            layout: new_layout(),
        }
    }
}

impl Document {
    /// Drop a graph node from the document: the graph, its placement, its
    /// selection membership and its viewer and inspector tabs. Every removal —
    /// an edit, an undo or a redo — comes through here, so no tab outlives its
    /// node.
    pub(crate) fn remove_node(&mut self, node_id: NodeId) -> DetachedNode {
        self.unplace(node_id);
        self.close_tabs(node_id);
        self.graph.detach_node(node_id)
    }

    /// [`Self::remove_node`] for a stub. A stub has no viewer: the load closes
    /// one, and no edit opens one on a node that cannot run.
    pub(crate) fn remove_stub(&mut self, node_id: NodeId) -> DetachedNode {
        self.unplace(node_id);
        self.stubs
            .remove(node_id)
            .expect("cannot remove a stub the document does not hold")
    }

    fn unplace(&mut self, node_id: NodeId) {
        self.main_view.node_placements.remove(&node_id);
        self.main_view.selected.remove(&ItemId::Node(node_id));
    }

    /// Close the tabs that show `node_id`: its viewer and an inspector pinned to it.
    fn close_tabs(&mut self, node_id: NodeId) {
        let shows = |tab: TabRef| {
            tab == TabRef::ImageViewer(node_id)
                || tab == TabRef::Inspector(InspectTarget::Node(node_id))
        };
        // `retain_tabs` rebuilds the whole node tree, so it runs only when
        // there is a tab to close.
        if self.layout.all_tabs().any(shows) {
            self.layout.retain_tabs(|tab| !shows(tab));
        }
    }

    /// Whether the graph is on a canvas — i.e. some pane's visible tab is the
    /// graph pane. What the scene projects, and what the canvas prepass gates
    /// on.
    pub(crate) fn shows_graph(&self) -> bool {
        self.layout.active_tabs().any(|tab| tab == TabRef::Graph)
    }

    /// Whether the document holds `node_id`, as a graph node or a stub.
    ///
    /// The retention rule for every `NodeId`-keyed cache that outlives the
    /// scene. Node ids are never reused, so a node absent here is gone for
    /// good, while absence from a scene means only that a node is off-screen
    /// or on a closed tab.
    pub(crate) fn holds_node(&self, node_id: NodeId) -> bool {
        self.graph.find(node_id).is_some() || self.stubs.contains(node_id)
    }

    /// Whether a tab may be open: the graph pane, `Preferences` and the
    /// selection's inspector always, a viewer exactly while its node is a
    /// preview node in the graph, and a pinned inspector while its node is in
    /// the graph.
    pub(crate) fn holds_tab(&self, tab: TabRef) -> bool {
        match tab {
            TabRef::Graph | TabRef::Preferences | TabRef::Inspector(InspectTarget::Selection) => {
                true
            }
            TabRef::ImageViewer(node_id) => self.holds_preview_node(node_id),
            TabRef::Inspector(InspectTarget::Node(node_id)) => self.graph.find(node_id).is_some(),
        }
    }

    /// [`Self::holds_node`] narrowed to preview nodes in the graph — what
    /// retains the value one published.
    ///
    /// Deliberately stricter than the shared rule rather than accidentally
    /// different, which is why it is spelled as a narrowing: the preview store
    /// holds textures up to 8192² RGBA8, so a value whose key is not a live
    /// preview node releases at the next sweep instead of riding on the
    /// node's own lifetime. One node backs one on-screen card, so the node's
    /// own id answers for the card without anything else alongside it.
    pub(crate) fn holds_preview_node(&self, node_id: NodeId) -> bool {
        is_preview_node(&self.graph, node_id)
    }

    /// Make a decoded document consistent with itself and `library`, and
    /// report each change: attach each stub whose func resolves again with
    /// the wiring that still fits, [`reconcile`](CoreGraph::reconcile) the
    /// graph and keep each node it detaches as a stub, then repair the view
    /// and the layout around the result. Afterwards [`Self::validate`] holds,
    /// and the graph compiles.
    pub(crate) fn reconcile(&mut self, library: &Library) -> LoadReport {
        let Self {
            graph,
            stubs,
            main_view,
            layout,
        } = self;
        let mut repairs = Vec::new();
        reattach_stubs(graph, stubs, library, &mut repairs);
        let reconciled = graph.reconcile(library);
        repairs.extend(reconciled.repairs.into_iter().map(DocumentRepair::Graph));
        for Detachment { record, reason } in reconciled.detached {
            repairs.push(DocumentRepair::Stubbed {
                node_id: record.node_id(),
                reason,
            });
            stubs.insert(record);
        }
        repair_view(main_view, graph, stubs, &mut repairs);
        repair_layout(layout, graph, &mut repairs);
        debug_assert!(
            self.validate().is_ok(),
            "a reconciled document is consistent: {:?}",
            self.validate()
        );
        LoadReport { repairs }
    }

    /// Check what [`Self::reconcile`] establishes that needs no library: the
    /// graph's structure, the stubs beside it, the view placing both, and the
    /// layout. A debug check: a document that fails it is a bug in the load
    /// or an edit.
    ///
    /// The layout is held to what [`new_layout`] builds: the graph tab
    /// pinned, which palantir's own load check already makes present, and
    /// this editor's dock seed.
    pub(crate) fn validate(&self) -> Result<(), DocumentValidationError> {
        self.graph.validate()?;
        if let Some(record) = self
            .stubs
            .iter()
            .find(|record| self.graph.find(record.node_id()).is_some())
        {
            return Err(DocumentValidationError::StubInGraph {
                node_id: record.node_id(),
            });
        }
        self.main_view
            .validate(&self.graph, &self.stubs)
            .map_err(|source| DocumentValidationError::MainView { source })?;

        let tab = self.layout.pinned();
        if tab != TabRef::Graph {
            return Err(DocumentValidationError::PinnedTab { tab });
        }
        if self.layout.seed() != new_layout().seed() {
            return Err(DocumentValidationError::ForeignDock);
        }

        for tab in self.layout.all_tabs() {
            if !self.holds_tab(tab) {
                return Err(DocumentValidationError::MissingTab { tab });
            }
        }
        // A file holds relative paths; memory resolves each where it enters, so a relative one
        // here would be read against the working directory.
        let mut relative = None;
        self.for_each_path(|node_id, path| {
            if relative.is_none() && !path.is_empty() && Path::new(path).is_relative() {
                relative = Some(DocumentValidationError::RelativePath {
                    node_id,
                    path: path.to_string(),
                });
            }
        });
        relative.map_or(Ok(()), Err)
    }

    /// Each path a const binding holds, the stubs' included, with the node it binds.
    pub(crate) fn for_each_path(&self, mut visit: impl FnMut(NodeId, &str)) {
        let graph = self.graph.bindings.iter();
        let stubs = self.stubs.iter().flat_map(|record| {
            record
                .bindings()
                .iter()
                .map(|entry| (&entry.port, &entry.binding))
        });
        for (port, binding) in graph.chain(stubs) {
            if let Binding::Const(value) = binding
                && let Some(paths) = value.as_fs_paths()
            {
                for path in paths {
                    visit(port.node_id, path);
                }
            }
        }
    }

    /// Put `map`'s answer in place of each path a const binding holds that it answers for, the
    /// stubs' included.
    pub(crate) fn map_paths(&mut self, mut map: impl FnMut(&str) -> Option<String>) {
        for binding in self.graph.bindings.values_mut() {
            map_binding(binding, &mut map);
        }
        for record in self.stubs.iter_mut() {
            let mut bindings = record.bindings().to_vec();
            let mut changed = false;
            for entry in &mut bindings {
                changed |= map_binding(&mut entry.binding, &mut map);
            }
            if changed {
                // A record is checked whole; changing a const keeps every check it passed.
                *record = DetachedNode::new(DetachedNodeParts {
                    node_id: record.node_id(),
                    node: record.node().clone(),
                    bindings,
                    subscriptions: record.subscriptions().to_vec(),
                })
                .expect("a record with other consts is as well formed as before");
            }
        }
    }
}

/// Put `map`'s answers in place in `binding`'s paths; whether it answered any.
fn map_binding(binding: &mut Binding, map: &mut impl FnMut(&str) -> Option<String>) -> bool {
    let paths: &mut [String] = match binding {
        Binding::Const(ConstValue::FsPath(path)) => slice::from_mut(path),
        Binding::Const(ConstValue::FsPaths(paths)) => paths,
        _ => return false,
    };
    let mut changed = false;
    for path in paths {
        if let Some(mapped) = map(path) {
            *path = mapped;
            changed = true;
        }
    }
    changed
}

impl From<CoreGraph> for Document {
    fn from(graph: CoreGraph) -> Self {
        let main_view = GraphView::new(&graph);
        Self {
            graph,
            stubs: Stubs::default(),
            main_view,
            layout: new_layout(),
        }
    }
}

/// Drop each stub saved under a graph node's id, and attach each stub whose
/// func resolves again with the wiring that still has both ends and lands on
/// an input the graph has not bound since.
fn reattach_stubs(
    graph: &mut CoreGraph,
    stubs: &mut Stubs,
    library: &Library,
    repairs: &mut Vec<DocumentRepair>,
) {
    for record in stubs.take_where(|record| graph.find(record.node_id()).is_some()) {
        repairs.push(DocumentRepair::DuplicateStub {
            node_id: record.node_id(),
        });
    }
    let resolving = stubs.take_where(|record| library.func_of(record.node()).is_ok());
    for record in &resolving {
        graph.insert(record.node_id(), record.node().clone());
        repairs.push(DocumentRepair::Reattached {
            node_id: record.node_id(),
        });
    }
    // Wiring after every node, so an edge between two stubs that both
    // resolve finds its other end whichever record holds it.
    for record in &resolving {
        let node_id = record.node_id();
        for entry in record.bindings() {
            let ends_held = graph.find(entry.port.node_id).is_some()
                && match &entry.binding {
                    Binding::Wire(source) => graph.find(source.node_id).is_some(),
                    Binding::Const(_) => true,
                };
            if ends_held && !graph.bindings.contains_key(&entry.port) {
                graph.bindings.insert(entry.port, entry.binding.clone());
            } else {
                repairs.push(DocumentRepair::StubBindingDropped {
                    node_id,
                    entry: entry.clone(),
                });
            }
        }
        for &subscription in record.subscriptions() {
            if graph.find(subscription.event.node_id).is_some()
                && graph.find(subscription.subscriber).is_some()
            {
                graph.subscribe(subscription.event, subscription.subscriber);
            } else {
                repairs.push(DocumentRepair::StubSubscriptionDropped {
                    node_id,
                    subscription,
                });
            }
        }
    }
}

/// Reset a bad viewport, drop each placement for no node or stub, move each
/// non-finite position to the origin, place each node or stub that has no
/// placement at the origin, repair the ties, and drop each selection entry
/// the view does not hold.
fn repair_view(
    main_view: &mut GraphView,
    graph: &CoreGraph,
    stubs: &Stubs,
    repairs: &mut Vec<DocumentRepair>,
) {
    if !main_view.viewport.is_valid() {
        main_view.viewport = Viewport::default();
        repairs.push(DocumentRepair::ViewportReset);
    }
    let placements = &mut main_view.node_placements;
    placements.retain(|&node_id, _| {
        let held = graph.find(node_id).is_some() || stubs.contains(node_id);
        if !held {
            repairs.push(DocumentRepair::PlacementDropped { node_id });
        }
        held
    });
    for (&node_id, placement) in placements.iter_mut() {
        if !placement.pos.is_finite() {
            placement.pos = Vec2::ZERO;
            repairs.push(DocumentRepair::PosReset { node_id });
        }
    }
    let mut unplaced: Vec<NodeId> = graph
        .iter()
        .map(|node| node.node_id)
        .chain(stubs.iter().map(DetachedNode::node_id))
        .filter(|node_id| !placements.contains_key(node_id))
        .collect();
    unplaced.sort_unstable();
    for node_id in unplaced {
        placements.insert(node_id, NodePlacement::default());
        repairs.push(DocumentRepair::Placed { node_id });
    }
    repair_ties(main_view, graph, repairs);
    let mut selected = std::mem::take(&mut main_view.selected);
    selected.retain(|&item| {
        let held = main_view.holds(item);
        if !held {
            repairs.push(DocumentRepair::SelectionDropped { item });
        }
        held
    });
    main_view.selected = selected;
}

/// Drop each route to a wire the graph does not hold, each step to no tie
/// and each repeat of a step, and each route left with no step; then drop
/// each tie no route names, move each non-finite tie to the origin, and free
/// each pinned angle outside `[0, 360)`.
fn repair_ties(main_view: &mut GraphView, graph: &CoreGraph, repairs: &mut Vec<DocumentRepair>) {
    let GraphView { ties, routes, .. } = main_view;
    routes.retain(|&wire, route| {
        if !wire.is_held_by(graph) {
            repairs.push(DocumentRepair::RouteDropped { wire });
            return false;
        }
        let mut steps: Vec<TieId> = Vec::with_capacity(route.iter().len());
        for &tie in route.iter() {
            if !ties.contains_key(&tie) {
                repairs.push(DocumentRepair::RouteStepDropped { wire, tie });
            } else if steps.contains(&tie) {
                repairs.push(DocumentRepair::RouteStepRepeated { wire, tie });
            } else {
                steps.push(tie);
            }
        }
        if steps.is_empty() {
            repairs.push(DocumentRepair::EmptyRouteDropped { wire });
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
            repairs.push(DocumentRepair::TieDropped { tie });
            return false;
        }
        if !placed.pos.is_finite() {
            placed.pos = Vec2::ZERO;
            repairs.push(DocumentRepair::TiePosReset { tie });
        }
        if !placed.angle.is_valid() {
            placed.angle = TieAngle::Auto;
            repairs.push(DocumentRepair::TieAngleFreed { tie });
        }
        true
    });
}

/// Reset a layout that pins another tab or belongs to another dock, else close
/// each viewer on a node that is no preview node in the graph.
fn repair_layout(
    layout: &mut DockState<TabRef>,
    graph: &CoreGraph,
    repairs: &mut Vec<DocumentRepair>,
) {
    if layout.pinned() != TabRef::Graph || layout.seed() != new_layout().seed() {
        *layout = new_layout();
        repairs.push(DocumentRepair::LayoutReset);
    } else {
        let stale: Vec<(TabRef, DocumentRepair)> = layout
            .all_tabs()
            .filter_map(|tab| {
                let repair = match tab {
                    TabRef::ImageViewer(node_id) => (!is_preview_node(graph, node_id))
                        .then_some(DocumentRepair::ViewerClosed { node_id }),
                    TabRef::Inspector(InspectTarget::Node(node_id)) => graph
                        .find(node_id)
                        .is_none()
                        .then_some(DocumentRepair::InspectorClosed { node_id }),
                    TabRef::Graph
                    | TabRef::Preferences
                    | TabRef::Inspector(InspectTarget::Selection) => None,
                }?;
                Some((tab, repair))
            })
            .collect();
        if !stale.is_empty() {
            layout.retain_tabs(|tab| stale.iter().all(|(closed, _)| *closed != tab));
            repairs.extend(stale.into_iter().map(|(_, repair)| repair));
        }
    }
}

/// Whether `graph` holds `node_id` as a preview node.
fn is_preview_node(graph: &CoreGraph, node_id: NodeId) -> bool {
    graph
        .find(node_id)
        .and_then(Node::func_id)
        .is_some_and(preview_sink::is_func)
}

/// A fresh dock: one pane showing the graph. `Graph` is the pinned
/// tab, so it refuses to close and the tree is never empty.
fn new_layout() -> DockState<TabRef> {
    DockState::new(DOCK_SEED, TabRef::Graph)
}

#[cfg(feature = "internals")]
pub(crate) mod internals;

#[cfg(test)]
mod tests;
