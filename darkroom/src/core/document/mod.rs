pub(crate) mod declaration;
pub(crate) mod error;
pub(crate) mod graph_revision;
pub(crate) mod graph_view;
pub(crate) mod tab_ref;
pub(crate) mod viewport;

use std::mem;

use palantir::DockState;
use scenarium::{Binding, DetachedNode, Graph as CoreGraph, Library, NodeId, NodeKind};
use serde::{Deserialize, Serialize};

use crate::core::document::error::DocumentValidationError;
use crate::core::document::graph_view::GraphView;
use crate::core::document::tab_ref::TabRef;
use crate::core::preview_sink;

/// The thing being edited: the authoring `Graph`, the editor view metadata
/// positioning it, and the pane layout showing it. The `Library` it resolves
/// against lives one level up on `App` (runtime-owned).
///
/// The fields are public because a caller that touches several wants them
/// borrowed together, and destructuring is what proves they are disjoint —
/// accessor methods would each borrow the whole `Document` and couldn't be
/// held at once.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Document {
    pub(crate) graph: CoreGraph,
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
            main_view: GraphView::default(),
            layout: new_layout(),
        }
    }
}

impl Document {
    /// Drop a node from both the graph and the view (its placement and any
    /// selection membership) — the one edit that has to touch both to leave
    /// the document consistent.
    pub(crate) fn remove_node(&mut self, node_id: NodeId) -> DetachedNode {
        self.main_view.item_placements.remove(&node_id);
        let detached = self.graph.detach_node(node_id);
        self.main_view.selected.remove(&node_id);
        detached
    }

    /// Whether the graph is on a canvas — i.e. some pane's visible tab is the
    /// graph pane. What the scene projects, and what the canvas prepass gates
    /// on.
    pub(crate) fn shows_graph(&self) -> bool {
        self.layout.active_tabs().any(|tab| tab == TabRef::Graph)
    }

    /// Whether the graph still holds `node_id`.
    ///
    /// **The** liveness question, and the single definition of it. Node ids
    /// are never reused, so a node absent here is gone for good — which is
    /// what makes this the retention rule for every `NodeId`-keyed cache that
    /// outlives the scene: the canvas geometry, the preview values, the open
    /// inspectors. Absence from a *scene* means only that a node is off-screen
    /// or on a closed tab; absence from here is permanent.
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
    /// node.
    ///
    /// Two neighbours are deliberately outside it: [`Self::reconcile_with_graph`]
    /// is the document repairing *itself* rather than a cache being swept, and
    /// must stay in the navigation phase so a stale tab cannot reach the record
    /// or a save; and `RunState::nodes` is a record of the last run rather than
    /// a cache derived from the document — see its field doc for the reasoning.
    pub(crate) fn holds_node(&self, node_id: NodeId) -> bool {
        self.graph.find(node_id).is_some()
    }

    /// [`Self::holds_node`] asked of a tab: the graph pane and `Preferences`
    /// always resolve, and a viewer tab dies exactly with its node. The single
    /// predicate behind [`Self::reconcile_with_graph`]'s fast path, its prune,
    /// and [`Self::validate`]'s tab check, so the three can't drift.
    pub(crate) fn holds_tab(&self, tab: TabRef) -> bool {
        match tab {
            TabRef::Graph | TabRef::Preferences => true,
            TabRef::ImageViewer(node_id) => self.holds_node(node_id),
        }
    }

    /// [`Self::holds_node`] narrowed to preview nodes — what retains the value one
    /// published.
    ///
    /// Deliberately stricter than the shared rule rather than accidentally
    /// different, which is why it is spelled as a narrowing: the preview store
    /// holds textures up to 8192² RGBA8, so a value whose key is not a live
    /// preview node releases at the next sweep instead of riding on the
    /// node's own lifetime. One node backs one on-screen card, so the node's
    /// own id answers for the card without anything else alongside it.
    pub(crate) fn holds_preview_node(&self, node_id: NodeId) -> bool {
        self.graph
            .find(node_id)
            .is_some_and(|node| match node.kind {
                NodeKind::Func(func_id) => preview_sink::is_func(func_id),
                NodeKind::Special(_) => false,
            })
    }

    /// Bring the editor's derived state back in line with the graph: drop
    /// tabs whose target vanished, collapsing panes that empty. The graph pane
    /// always survives — the graph and `main_view` both always exist — so this
    /// only ever prunes viewer tabs whose node is gone.
    ///
    /// Runs every frame in the navigation phase, right after undo/redo and the
    /// intent drain, so a stale tab can never reach a save.
    pub(crate) fn reconcile_with_graph(&mut self) {
        // Common case: every tab still resolves, so touch nothing.
        // `retain_tabs` rebuilds the whole node tree through `normalize`,
        // allocating as it goes, and must not run on a frame with nothing to
        // prune.
        if self.layout.all_tabs().all(|tab| self.holds_tab(tab)) {
            return;
        }
        // The retain wants `layout` mutably while the predicate reads the rest
        // of the document, which one `&mut self` cannot hand out. Moving the
        // layout out for the call splits them: the predicate borrows a
        // `Document` whose layout is a placeholder, and it never asks about
        // one. Cold path only — the fast path above already returned.
        let mut layout = mem::replace(&mut self.layout, new_layout());
        layout.retain_tabs(|tab| self.holds_tab(tab));
        self.layout = layout;
    }

    /// Full structural validation for untrusted documents.
    ///
    /// The layout is held to what [`new_layout`] builds: the graph tab
    /// pinned, which palantir's own load check already makes present, and
    /// this editor's dock seed.
    pub(crate) fn validate(&self) -> Result<(), DocumentValidationError> {
        self.graph.validate()?;
        self.main_view
            .validate(&self.graph)
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

    /// Hold every binding and subscription to the ports and events its nodes'
    /// funcs declare in `library` — the half of validation [`Self::validate`]
    /// cannot do, having no library. Same rule as the edit gate's, in
    /// [`declaration`].
    pub(crate) fn validate_declarations(
        &self,
        library: &Library,
    ) -> Result<(), DocumentValidationError> {
        let graph = &self.graph;
        for (&port, binding) in &graph.bindings {
            if let Some(node) = graph.find(port.node_id)
                && !declaration::admits_input(node, library, port.port_idx)
            {
                return Err(DocumentValidationError::UndeclaredInput { port });
            }
            if let Binding::Bind(source) = binding
                && let Some(producer) = graph.find(source.node_id)
                && !declaration::admits_output(producer, library, source.port_idx)
            {
                return Err(DocumentValidationError::UndeclaredOutput { port: *source });
            }
        }
        for subscription in graph.subscriptions() {
            if let Some(emitter) = graph.find(subscription.emitter)
                && !declaration::admits_event(emitter, library, subscription.event_idx)
            {
                return Err(DocumentValidationError::UndeclaredEvent { subscription });
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
            main_view,
            layout: new_layout(),
        }
    }
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
    use glam::Vec2;
    use palantir::DockOperation;
    use scenarium::Node;

    use super::*;
    use crate::core::document::internals::DocFixture;

    /// Every `NodeId`-keyed cache sweeps against one rule, and the two
    /// narrowings of it are strict subsets — so a node that is gone is gone by
    /// all of them at once, and nothing can be retained by one sweep while
    /// another has released it.
    ///
    /// Re-diverging the predicates is what this catches.
    #[test]
    fn node_liveness_is_one_rule_with_two_declared_narrowings() {
        let mut fixture = DocFixture::default();
        let preview_func = preview_sink::func(Arc::default());
        fixture.library.add(preview_func.clone());
        let preview = fixture.doc.graph.add(Node::from(&preview_func));
        let plain = fixture.stub_at(Vec2::ZERO);
        let mut doc = fixture.doc;

        // The shared rule answers for both kinds.
        for node in [preview, plain] {
            assert!(doc.holds_node(node), "the document holds {node:?}");
        }
        assert!(doc.holds_preview_node(preview));
        assert!(
            !doc.holds_preview_node(plain),
            "a stub node retains no published value"
        );

        // The tab lift is the same rule asked of a tab: a viewer dies exactly
        // with its node, and the two non-node tabs never do.
        assert!(doc.holds_tab(TabRef::ImageViewer(preview)));
        assert!(doc.holds_tab(TabRef::Graph));
        assert!(doc.holds_tab(TabRef::Preferences));

        doc.graph.detach_node(preview);
        assert!(!doc.holds_node(preview), "the rule says gone");
        assert!(
            !doc.holds_preview_node(preview),
            "and so does the narrowing"
        );
        assert!(
            !doc.holds_tab(TabRef::ImageViewer(preview)),
            "and so does the tab lift"
        );
        assert!(doc.holds_node(plain), "the surviving node is untouched");
    }

    #[test]
    fn dock_layout_round_trips_as_ron() {
        use palantir::{DockDrop, SplitSide};

        let mut doc = DocFixture::sample().doc;
        let node_id = doc.graph.iter().next().unwrap().id;
        let primary = doc.layout.primary().id;
        doc.layout.find_or_insert(TabRef::Preferences, primary);
        doc.layout
            .find_or_insert(TabRef::ImageViewer(node_id), primary);
        // A split pane too, so the whole tree shape round-trips — not
        // just a flat strip.
        doc.layout.apply(DockOperation::MoveTab {
            tab: TabRef::ImageViewer(node_id),
            to: DockDrop::Split {
                group: primary,
                side: SplitSide::Right,
            },
        });
        let bytes = SerdeFormat::Ron
            .serialize(&doc)
            .expect("serialize with dock layout");
        let back: Document = SerdeFormat::Ron.deserialize(&bytes).expect("deserialize");
        back.validate().expect("round-tripped document is valid");
        assert_eq!(
            back.layout, doc.layout,
            "the split tree (groups, focus, ratio) round-trips through RON"
        );
    }

    /// The sample validates, and a layout that pins another tab, or was
    /// saved under another dock seed, does not — each would pass palantir's
    /// own load check.
    #[test]
    fn validation_holds_the_layout_to_the_editors_dock() {
        let mut doc = DocFixture::sample().doc;
        doc.validate().unwrap();

        doc.layout = DockState::new(DOCK_SEED, TabRef::Preferences);
        assert!(matches!(
            doc.validate(),
            Err(DocumentValidationError::PinnedTab {
                tab: TabRef::Preferences
            })
        ));

        doc.layout = DockState::new("another.dock", TabRef::Graph);
        assert!(matches!(
            doc.validate(),
            Err(DocumentValidationError::ForeignDock)
        ));
    }
}
