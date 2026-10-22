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
pub(crate) mod load_report;
pub(crate) mod stubs;
pub(crate) mod tab_ref;
pub(crate) mod viewport;

use glam::Vec2;
use palantir::DockState;
use scenarium::{Binding, DetachedNode, Detachment, Graph as CoreGraph, Library, Node, NodeId};
use serde::{Deserialize, Serialize};

use crate::core::document::error::DocumentValidationError;
use crate::core::document::graph_view::{GraphView, ItemPlacement};
use crate::core::document::load_report::{DocumentRepair, LoadReport};
use crate::core::document::stubs::Stubs;
use crate::core::document::tab_ref::TabRef;
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
#[derive(Debug, PartialEq, Serialize, Deserialize)]
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
    /// selection membership and its viewer tab. Every removal — an edit, an
    /// undo or a redo — comes through here, so no tab outlives its node.
    pub(crate) fn remove_node(&mut self, node_id: NodeId) -> DetachedNode {
        self.unplace(node_id);
        self.close_viewer(node_id);
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
        self.main_view.item_placements.remove(&node_id);
        self.main_view.selected.remove(&node_id);
    }

    fn close_viewer(&mut self, node_id: NodeId) {
        let viewer = TabRef::ImageViewer(node_id);
        // `retain_tabs` rebuilds the whole node tree, so it runs only when
        // there is a tab to close.
        if self.layout.all_tabs().any(|tab| tab == viewer) {
            self.layout.retain_tabs(|tab| tab != viewer);
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
    /// **The** liveness question, and the single definition of it. Node ids
    /// are never reused, so a node absent here is gone for good — which is
    /// what makes this the retention rule for every `NodeId`-keyed cache that
    /// outlives the scene: the canvas geometry, the open inspectors. Absence
    /// from a *scene* means only that a node is off-screen or on a closed tab;
    /// absence from here is permanent.
    ///
    /// **Who asks it, and where the next one goes.** Two sweeps run from
    /// `App::update`, once a frame, and between them their bodies name every
    /// cache: [`MainWindow::reconcile`](crate::gui::main_window::MainWindow::reconcile)
    /// for what the window owns, and `RunState::sync` for the published preview
    /// values (through [`Self::holds_preview_node`]). A new cache derived from
    /// the document joins one of those two bodies rather than earning a third
    /// call site, which is what keeps the list in code that a reader can open
    /// rather than in this comment.
    ///
    /// `MainWindow::image_viewers` rides the same pass but is swept against the
    /// *layout*, since a viewer's framing dies with its tab rather than with its
    /// node. `RunState::nodes` is a record of the last run rather than a cache
    /// derived from the document — see its field doc for the reasoning.
    pub(crate) fn holds_node(&self, node_id: NodeId) -> bool {
        self.graph.find(node_id).is_some() || self.stubs.contains(node_id)
    }

    /// Whether a tab may be open: the graph pane and `Preferences` always,
    /// and a viewer exactly while its node is a preview node in the graph.
    pub(crate) fn holds_tab(&self, tab: TabRef) -> bool {
        match tab {
            TabRef::Graph | TabRef::Preferences => true,
            TabRef::ImageViewer(node_id) => self.holds_preview_node(node_id),
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
        Ok(())
    }
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
/// placement at the origin, and drop each selection entry with no placement.
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
    let placements = &mut main_view.item_placements;
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
        .map(|node| node.id)
        .chain(stubs.iter().map(DetachedNode::node_id))
        .filter(|node_id| !placements.contains_key(node_id))
        .collect();
    unplaced.sort_unstable();
    for node_id in unplaced {
        placements.insert(node_id, ItemPlacement::default());
        repairs.push(DocumentRepair::Placed { node_id });
    }
    main_view.selected.retain(|&node_id| {
        let placed = main_view.item_placements.contains_key(&node_id);
        if !placed {
            repairs.push(DocumentRepair::SelectionDropped { node_id });
        }
        placed
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
        let stale: Vec<NodeId> = layout
            .all_tabs()
            .filter_map(|tab| match tab {
                TabRef::ImageViewer(node_id) if !is_preview_node(graph, node_id) => Some(node_id),
                _ => None,
            })
            .collect();
        if !stale.is_empty() {
            layout.retain_tabs(
                |tab| !matches!(tab, TabRef::ImageViewer(node_id) if stale.contains(&node_id)),
            );
            repairs.extend(
                stale
                    .into_iter()
                    .map(|node_id| DocumentRepair::ViewerClosed { node_id }),
            );
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

#[cfg(test)]
pub(crate) mod internals;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use common::SerdeFormat;
    use palantir::DockOperation;

    use super::*;
    use crate::core::document::internals::DocumentFixture;

    /// Every `NodeId`-keyed cache sweeps against one rule, and the two
    /// narrowings of it are strict subsets — so a node that is gone is gone by
    /// all of them at once, and nothing can be retained by one sweep while
    /// another has released it.
    ///
    /// Re-diverging the predicates is what this catches.
    #[test]
    fn node_liveness_is_one_rule_with_two_declared_narrowings() {
        let mut fixture = DocumentFixture::default();
        let preview_func = preview_sink::func(Arc::default());
        fixture.library.add(preview_func.clone());
        let preview = fixture.document.graph.add(Node::from(&preview_func));
        let plain = fixture.stub_at(Vec2::ZERO);
        let mut document = fixture.document;

        // The shared rule answers for a graph node and a stub alike.
        for node in [preview, plain] {
            assert!(document.holds_node(node), "the document holds {node:?}");
        }
        assert!(document.holds_preview_node(preview));
        assert!(
            !document.holds_preview_node(plain),
            "a stub retains no published value"
        );
        assert!(
            !document.holds_tab(TabRef::ImageViewer(plain)),
            "a stub has no viewer"
        );

        // The tab lift is the same rule asked of a tab: a viewer dies exactly
        // with its node, and the two non-node tabs never do.
        assert!(document.holds_tab(TabRef::ImageViewer(preview)));
        assert!(document.holds_tab(TabRef::Graph));
        assert!(document.holds_tab(TabRef::Preferences));

        document.graph.detach_node(preview);
        assert!(!document.holds_node(preview), "the rule says gone");
        assert!(
            !document.holds_preview_node(preview),
            "and so does the narrowing"
        );
        assert!(
            !document.holds_tab(TabRef::ImageViewer(preview)),
            "and so does the tab lift"
        );
        assert!(
            document.holds_node(plain),
            "the surviving node is untouched"
        );
    }

    /// A removal closes the node's viewer tab: every removal, by an edit, an
    /// undo or a redo, goes through `remove_node`, so no tab outlives its node.
    #[test]
    fn removing_a_node_closes_its_viewer() {
        let mut fixture = DocumentFixture::sample();
        let node_id = fixture.add(&preview_sink::func(Arc::default()));
        let mut document = fixture.document;
        let primary = document.layout.primary().id;
        document
            .layout
            .find_or_insert(TabRef::ImageViewer(node_id), primary);

        document.remove_node(node_id);
        assert!(
            !document
                .layout
                .all_tabs()
                .any(|tab| tab == TabRef::ImageViewer(node_id))
        );
        document.validate().expect("no tab names the removed node");
    }

    /// The load repairs the view and the layout around the graph, in order,
    /// and reports each repair: a bad viewport resets, a placement for no node
    /// goes, a non-finite position and a missing placement go to the origin, a
    /// selection entry with no placement goes, and a viewer on a node that is
    /// no preview node closes. A layout that pins another tab resets whole.
    #[test]
    fn reconcile_repairs_the_view_and_the_layout() {
        let mut fixture = DocumentFixture::default();
        let shown = fixture.node_at(Vec2::new(f32::NAN, 0.0));
        let unplaced = fixture.node_at(Vec2::ZERO);
        let DocumentFixture {
            mut document,
            library,
        } = fixture;
        let ghost = NodeId::unique();
        document.main_view.viewport = Viewport {
            pan: Vec2::ZERO,
            zoom: 0.0,
        };
        document.main_view.item_placements.remove(&unplaced);
        document
            .main_view
            .item_placements
            .insert(ghost, ItemPlacement::default());
        document.main_view.selected = [unplaced].into_iter().collect();
        let primary = document.layout.primary().id;
        document
            .layout
            .find_or_insert(TabRef::ImageViewer(shown), primary);

        let report = document.reconcile(&library);
        assert_eq!(
            report.repairs,
            [
                DocumentRepair::ViewportReset,
                DocumentRepair::PlacementDropped { node_id: ghost },
                DocumentRepair::PosReset { node_id: shown },
                DocumentRepair::Placed { node_id: unplaced },
                DocumentRepair::ViewerClosed { node_id: shown },
            ]
        );
        assert_eq!(document.main_view.item_placements[&shown].pos, Vec2::ZERO);
        assert_eq!(
            document.main_view.item_placements[&unplaced].pos,
            Vec2::ZERO
        );
        assert!(
            document.main_view.selected.contains(&unplaced),
            "placed before the selection is checked, so it stays selected"
        );
        document.validate().expect("a reconciled document is valid");
        assert!(
            document.reconcile(&library).is_empty(),
            "nothing is left to repair"
        );

        document.layout = DockState::new(DOCK_SEED, TabRef::Preferences);
        assert_eq!(
            document.reconcile(&library).repairs,
            [DocumentRepair::LayoutReset]
        );
        assert_eq!(document.layout, new_layout());
    }

    #[test]
    fn dock_layout_round_trips_as_ron() {
        use palantir::{DockDrop, SplitSide};

        let mut fixture = DocumentFixture::sample();
        let node_id = fixture.add(&preview_sink::func(Arc::default()));
        let mut document = fixture.document;
        let primary = document.layout.primary().id;
        document.layout.find_or_insert(TabRef::Preferences, primary);
        document
            .layout
            .find_or_insert(TabRef::ImageViewer(node_id), primary);
        // A split pane too, so the whole tree shape round-trips — not
        // just a flat strip.
        document.layout.apply(DockOperation::MoveTab {
            tab: TabRef::ImageViewer(node_id),
            to: DockDrop::Split {
                group: primary,
                side: SplitSide::Right,
            },
        });
        let bytes = SerdeFormat::Ron
            .serialize(&document)
            .expect("serialize with dock layout");
        let back: Document = SerdeFormat::Ron.deserialize(&bytes).expect("deserialize");
        back.validate().expect("round-tripped document is valid");
        assert_eq!(
            back.layout, document.layout,
            "the split tree (groups, focus, ratio) round-trips through RON"
        );
    }

    /// The sample validates, and a layout that pins another tab, or was
    /// saved under another dock seed, does not — each would pass palantir's
    /// own load check.
    #[test]
    fn validation_holds_the_layout_to_the_editors_dock() {
        let mut document = DocumentFixture::sample().document;
        document.validate().unwrap();

        document.layout = DockState::new(DOCK_SEED, TabRef::Preferences);
        assert!(matches!(
            document.validate(),
            Err(DocumentValidationError::PinnedTab {
                tab: TabRef::Preferences
            })
        ));

        document.layout = DockState::new("another.dock", TabRef::Graph);
        assert!(matches!(
            document.validate(),
            Err(DocumentValidationError::ForeignDock)
        ));
    }
}
