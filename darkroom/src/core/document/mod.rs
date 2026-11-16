//! The document being edited, and how a decoded one is made consistent.
//!
//! # Decisions
//!
//! - **A stub is scenarium's.** A node its graph cannot hold is kept in the graph beside its
//!   nodes as a [`scenarium::Stub`], with every edge that touched it and why, so your settings
//!   survive a library that lost its func; the load is [`CoreGraph::reconcile`]. Refused:
//!   dropping them, as the settings are lost; remapping by port name, as a value whose meaning
//!   changed still goes through.
//! - **A graph with a stub runs the rest.** A consumer a stub fed sees an
//!   unbound input, as with a disabled producer. Refused: one stub refusing
//!   every run.
//! - **A stub can be selected, moved and deleted, not edited or copied.** It
//!   has no ports and a read-only title, and Duplicate skips it: a copy of a
//!   node that cannot run has little use.
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
mod layout_file;
pub(crate) mod load_report;
pub(crate) mod route;
pub(crate) mod tab_ref;
pub(crate) mod tie;
pub(crate) mod viewport;
pub(crate) mod wire_key;

use std::collections::BTreeSet;
use std::slice;

use glam::Vec2;
use palantir::DockState;
use scenarium::{
    ConstValue, DetachedNode, Graph as CoreGraph, Library, Node, NodeId, PathForm, ProgramId, Stub,
};
use serde::{Deserialize, Serialize};

use crate::core::document::error::DocumentValidationError;
use crate::core::document::graph_view::{GraphView, NodePlacement};
use crate::core::document::inspect_target::InspectTarget;
use crate::core::document::item_id::ItemId;
use crate::core::document::load_report::{DocumentRepair, ForeignPath, LoadReport};
use crate::core::document::route::Route;
use crate::core::document::tab_ref::TabRef;
use crate::core::document::tie::{TieAngle, TieId};
use crate::core::document::viewport::Viewport;
use crate::core::preview_sink;

/// The thing being edited: the authoring `Graph` with the stubs it keeps, the editor view
/// metadata positioning both, and the pane layout showing them. The `Library` they resolve
/// against lives one level up on `App`.
///
/// A decoded document is made consistent once, by [`Self::reconcile`] at
/// load; every edit after keeps it so, and nothing after re-checks it.
///
/// The fields are public because a caller that touches several wants them borrowed together, and
/// destructuring proves they are disjoint, where accessor methods would each borrow the whole
/// `Document`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Document {
    /// Holds only nodes the library resolves, with only admitted wiring, and keeps each node the
    /// library cannot resolve as a stub.
    pub(crate) graph: CoreGraph,
    /// Places every graph node and every stub.
    pub(crate) main_view: GraphView,
    /// The pane arrangement: open tabs grouped into split panes, plus the focused group.
    /// Mutated only through a `DockOperation`, none of which is undoable.
    #[serde(default = "new_layout", with = "layout_file")]
    pub(crate) layout: DockState<TabRef>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            graph: CoreGraph::default(),
            main_view: GraphView::default(),
            layout: new_layout(),
        }
    }
}

impl Document {
    /// Drop a graph node from the document: the graph, its placement, its selection membership
    /// and its viewer and inspector tabs. Every removal (an edit, an undo or a redo) comes through
    /// here, so no tab outlives its node.
    pub(crate) fn remove_node(&mut self, node_id: NodeId) -> DetachedNode {
        self.unplace(node_id);
        self.close_tabs(node_id);
        self.graph.detach_node(node_id)
    }

    /// [`Self::remove_node`] for a stub. A stub has no viewer: the load closes
    /// one, and no edit opens one on a node that cannot run.
    pub(crate) fn remove_stub(&mut self, node_id: NodeId) -> Stub {
        self.unplace(node_id);
        self.graph
            .remove_stub(node_id)
            .expect("cannot remove a stub the document does not hold")
    }

    fn unplace(&mut self, node_id: NodeId) {
        self.main_view.node_placements.remove(&node_id);
        self.main_view.selected.remove(&ItemId::Node(node_id));
    }

    /// Close the tabs that show `node_id`: its viewer and an inspector pinned to it.
    fn close_tabs(&mut self, node_id: NodeId) {
        let shows = |tab: TabRef| {
            tab == TabRef::ImageViewer(ProgramId::from(node_id))
                || tab == TabRef::Inspector(InspectTarget::Node(node_id))
        };
        // `retain_tabs` rebuilds the whole node tree, so it runs only when
        // there is a tab to close.
        if self.layout.all_tabs().any(shows) {
            self.layout.retain_tabs(|tab| !shows(tab));
        }
    }

    /// Whether the document holds `node_id`, as a graph node or a stub.
    ///
    /// The retention rule for every `NodeId`-keyed cache that outlives the
    /// scene. Node ids are never reused, so a node absent here is gone for
    /// good, while absence from a scene means only that a node is off-screen
    /// or on a closed tab.
    pub(crate) fn holds_node(&self, node_id: NodeId) -> bool {
        self.graph.holds(node_id)
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
            TabRef::ImageViewer(program_id) => self.holds_preview_node(program_id),
            TabRef::Inspector(InspectTarget::Node(node_id)) => self.graph.find(node_id).is_some(),
        }
    }

    /// Whether `program_id` names a preview node the document holds: a node of the main graph,
    /// held there as a preview node. The retention of the value one published. Stricter than
    /// [`Self::holds_node`]: the preview store holds textures up to 8192² RGBA8, so a value whose
    /// key is not a live preview node releases at the next sweep instead of riding on the node's
    /// own lifetime.
    pub(crate) fn holds_preview_node(&self, program_id: ProgramId) -> bool {
        program_id.path().is_main() && is_preview_node(&self.graph, program_id.node_id())
    }

    /// Make a decoded document consistent with itself and `library`, and
    /// report each change: [`reconcile`](CoreGraph::reconcile) the graph, which
    /// keeps each node it cannot hold as a stub and attaches each stub that
    /// resolves again, then repair the view and the layout around the result.
    /// Afterwards [`Self::validate`] holds, and the graph compiles.
    pub(crate) fn reconcile(&mut self, library: &Library) -> LoadReport {
        let Self {
            graph,
            main_view,
            layout,
        } = self;
        let mut repairs: Vec<DocumentRepair> = graph
            .reconcile(library)
            .repairs
            .into_iter()
            .map(DocumentRepair::Graph)
            .collect();
        repair_view(main_view, graph, &mut repairs);
        repair_layout(layout, graph, &mut repairs);
        debug_assert!(
            self.validate().is_ok(),
            "a reconciled document is consistent: {:?}",
            self.validate()
        );
        let mut foreign_paths = Vec::new();
        self.for_each_path(|node_id, path| {
            if PathForm::of(path) == PathForm::Foreign {
                foreign_paths.push(ForeignPath {
                    node_id,
                    path: path.to_string(),
                });
            }
        });
        LoadReport {
            repairs,
            foreign_paths,
        }
    }

    /// Check what [`Self::reconcile`] establishes that needs no library: the
    /// graph's structure with its stubs, the view placing both, and the
    /// layout. A debug check: a document that fails it is a bug in the load
    /// or an edit.
    ///
    pub(crate) fn validate(&self) -> Result<(), DocumentValidationError> {
        self.graph.validate()?;
        self.main_view
            .validate(&self.graph)
            .map_err(|source| DocumentValidationError::MainView { source })?;

        for tab in self.layout.all_tabs() {
            if !self.holds_tab(tab) {
                return Err(DocumentValidationError::MissingTab { tab });
            }
        }
        // A file holds relative paths; memory resolves each where it enters, so a relative one
        // here would be read against the working directory.
        let mut relative = None;
        self.for_each_path(|node_id, path| {
            if relative.is_none() && PathForm::of(path) == PathForm::Relative {
                relative = Some(DocumentValidationError::RelativePath {
                    node_id,
                    path: path.to_string(),
                });
            }
        });
        relative.map_or(Ok(()), Err)
    }

    /// Each path a constant holds, the stubs' included, with the node it binds.
    pub(crate) fn for_each_path(&self, mut visit: impl FnMut(NodeId, &str)) {
        for (port, value) in self.graph.constants() {
            for path in value.as_fs_paths().into_iter().flatten() {
                visit(port.node_id, path);
            }
        }
    }

    /// Put `map`'s answer in place of each path a constant holds that it answers for, the stubs'
    /// included.
    pub(crate) fn map_paths(&mut self, mut map: impl FnMut(&str) -> Option<String>) {
        for value in self.graph.constants_mut() {
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

impl From<CoreGraph> for Document {
    fn from(graph: CoreGraph) -> Self {
        let main_view = GraphView::new(&graph);
        Self {
            graph,
            main_view,
            layout: new_layout(),
        }
    }
}

/// Reset a bad viewport, drop each placement for no node or stub, move each
/// non-finite position to the origin, place each node or stub that has no
/// placement at the origin, repair the ties, and drop each selection entry
/// the view does not hold.
fn repair_view(main_view: &mut GraphView, graph: &CoreGraph, repairs: &mut Vec<DocumentRepair>) {
    if !main_view.viewport.is_valid() {
        main_view.viewport = Viewport::default();
        repairs.push(DocumentRepair::ViewportReset);
    }
    let placements = &mut main_view.node_placements;
    placements.retain(|&node_id, _| {
        let held = graph.holds(node_id);
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
        .chain(graph.stubs().map(Stub::node_id))
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

/// Close each viewer on a node that is no preview node in the graph, and each inspector on a
/// node the graph does not hold.
fn repair_layout(
    layout: &mut DockState<TabRef>,
    graph: &CoreGraph,
    repairs: &mut Vec<DocumentRepair>,
) {
    let stale: Vec<(TabRef, DocumentRepair)> = layout
        .all_tabs()
        .filter_map(|tab| {
            let repair = match tab {
                TabRef::ImageViewer(program_id) => (!program_id.path().is_main()
                    || !is_preview_node(graph, program_id.node_id()))
                .then_some(DocumentRepair::ViewerClosed { program_id }),
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

/// Whether `graph` holds `node_id` as a preview node.
fn is_preview_node(graph: &CoreGraph, node_id: NodeId) -> bool {
    graph
        .find(node_id)
        .and_then(Node::func_id)
        .is_some_and(preview_sink::is_func)
}

/// A fresh dock: one pane showing the graph, the pinned tab, so the tree is never empty.
fn new_layout() -> DockState<TabRef> {
    DockState::new(TabRef::Graph)
}

#[cfg(feature = "internals")]
pub(crate) mod internals;

#[cfg(test)]
mod tests;
