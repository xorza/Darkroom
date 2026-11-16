pub(crate) mod dock_panes;
pub(crate) mod menu_bar;
pub(crate) mod status_bar;

use std::collections::{BTreeMap, HashMap};

use palantir::{
    Chrome, Configure, DockOperation, DockView, Panel, SizeSpec, TabOverflow, Ui, VAlign, WidgetId,
};
use scenarium::{GraphId, ProgramId};

use crate::core::document::tab_ref::TabRef;
use crate::core::open_document::OpenDocument;
use crate::gui::frame::doc_ctx::DocCtx;
use crate::gui::frame::framing::Framing;
use crate::gui::frame::stage::Stage;
use crate::gui::frame::window_parts::WindowParts;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::main_window::dock_panes::DockPanes;
use crate::gui::pane::graph_ui::GraphUi;
use crate::gui::pane::image_viewer::ImageViewer;

/// Smallest a dock pane can be squeezed on its split axis, in logical
/// px.
const MIN_PANE: f32 = 220.0;

/// Top of darkroom's UI tree: the surround (menu bar, status bar) around
/// the dock, plus the per-view state the dock's panes render into. The
/// pane *machinery* — strips, splits, drag-docking — is
/// [`DockView`]'s, and what each tab kind looks like is
/// [`DockPanes`]'s. Adding a new pane *kind* is a new arm there.
///
/// Each entry point takes a [`DocCtx`], the frame's world *and* the document it is showing,
/// settled by the time the caller reaches it; each graph pane narrows it to its graph's
/// [`GraphCtx`]. In a [`Stage`], on the input stage and on the record path alike.
#[derive(Default, Debug)]
pub(crate) struct MainWindow {
    /// One canvas per graph a tab shows, made when the tab first shows and dropped when its
    /// last tab closes.
    pub(crate) graph_panes: BTreeMap<GraphId, GraphUi>,
    /// One image-viewer navigation state per rendered viewer tab
    /// ([`TabRef::ImageViewer`]), keyed by the preview program node it shows. Textures remain
    /// centralized in the preview store.
    pub(crate) image_viewers: HashMap<ProgramId, ImageViewer>,
    /// The dock's op sink for one frame, kept so the navigation scan and the record reuse one
    /// buffer.
    dock_ops: Vec<DockOperation<TabRef>>,
}

impl MainWindow {
    /// The dock's id, which its scan and its record share and every pane's id derives from.
    pub(crate) fn dock_wid() -> WidgetId {
        WidgetId::from_hash("darkroom.dock")
    }

    /// Navigation scan: surface tab activate/close/drag-drop from *last* frame's chip responses.
    /// Runs at the top of the frame so a switch applies before the record: no first-frame gap.
    ///
    /// The panes' own input reads are the input stage's, over the arrangement this phase's drain
    /// settles. The ops are handed to the frame's queue, where dock ops travel beside graph
    /// edits, stay out of undo, and are validated before a save.
    pub(crate) fn scan_navigation(&mut self, ui: &mut Ui, stage: &mut Stage<'_, DocCtx<'_>>) {
        let ops = &mut self.dock_ops;
        ops.clear();
        let layout = &stage.ctx.document().layout;
        DockView::scan(ui, Self::dock_wid(), layout, ops);
        for op in ops.drain(..) {
            stage.out.push_view(op);
        }
    }

    /// Input stage: input-derived graph mutations for the
    /// already-settled active graph, plus the per-pane visibility reconcile
    /// that has to happen before them.
    ///
    /// A pane that became visible this frame owes a relayout, since a canvas that has never
    /// recorded has no cached geometry to draw its first frame from. A pane that *vanished*
    /// needs no pass and is never visited here.
    pub(crate) fn input(&mut self, ui: &mut Ui, stage: &mut Stage<'_, DocCtx<'_>>) {
        let display = ui.display();
        // Reached from `active_tabs`, so a pane is showing the graph by construction. No other
        // kind derives a document mutation from input: preferences edits go through their own
        // widgets, and a viewer only navigates its own texture.
        for graph in stage
            .ctx
            .document()
            .layout
            .active_tabs()
            .filter_map(TabRef::graph)
        {
            let ctx = GraphCtx::of(stage.ctx, graph, display);
            self.graph_panes
                .entry(graph)
                .or_default()
                .input(ui, &mut stage.with(ctx));
        }
    }

    /// Ready the panes on screen for a record pass that no input stage ran just before; see
    /// [`GraphUi::settle`].
    pub(crate) fn settle(&mut self, ui: &Ui, stage: &mut Stage<'_, DocCtx<'_>>) {
        let display = ui.display();
        // No other kind keeps anything a second pass reads from the rects of the first.
        for graph in stage
            .ctx
            .document()
            .layout
            .active_tabs()
            .filter_map(TabRef::graph)
        {
            let ctx = GraphCtx::of(stage.ctx, graph, display);
            self.graph_panes
                .entry(graph)
                .or_default()
                .settle(ui, &mut stage.with(ctx));
        }
    }

    pub(crate) fn record(
        &mut self,
        ui: &mut Ui,
        stage: &mut Stage<'_, DocCtx<'_>>,
        parts: WindowParts<'_>,
    ) {
        let WindowParts {
            preferences,
            status,
        } = parts;
        let ctx = stage.ctx;
        let fill = ctx.theme().colors.surround_bg;
        let Self {
            graph_panes,
            image_viewers,
            dock_ops,
        } = self;
        Panel::vstack().size(SizeSpec::FILL).show(ui, |ui| {
            Panel::hstack()
                .id_salt("surround_row")
                .size(SizeSpec::FILL_W)
                .child_align(VAlign::Bottom)
                .chrome(Chrome::new(fill))
                .show(ui, |ui| {
                    menu_bar::show(ui, stage);
                });
            let mut panes = DockPanes {
                stage: stage.with(ctx),
                graph_panes,
                image_viewers,
                preferences,
            };
            dock_ops.clear();
            DockView::emitting(Self::dock_wid(), &ctx.document().layout, dock_ops)
                .min_pane(MIN_PANE)
                .overflow(TabOverflow::Menu)
                .show(ui, &mut panes);
            // Ratio drags and split-menu picks.
            for op in dock_ops.drain(..) {
                stage.out.push_view(op);
            }
            status_bar::show(ui, stage, status);
        });
    }

    /// Ask the active graph's canvas to frame `framing` in its next input stage.
    pub(crate) fn frame(&mut self, active: GraphId, framing: Framing) {
        self.graph_panes
            .entry(active)
            .or_default()
            .request_framing(framing);
    }

    /// Release everything this window caches for a subject the document has
    /// stopped holding: a canvas whose graph no tab shows, each canvas's `NodeId`-keyed tables
    /// (see [`GraphUi::sweep`]), swept only when its graph's revision moved, and the per-tab
    /// viewer state.
    ///
    /// Driven from `App::sweep` once a frame, beside the preview store's sweep; a new cache
    /// joins them rather than earning its own call site.
    pub(crate) fn reconcile(&mut self, open: &OpenDocument) {
        let document = open.document();
        self.graph_panes.retain(|&graph, _| {
            document
                .layout
                .all_tabs()
                .any(|tab| tab.graph() == Some(graph))
        });
        for (&graph, pane) in &mut self.graph_panes {
            pane.sweep(document.graphs.graph(graph), open.graph_revision(graph));
        }
        // Scoped to the tab, not the node: a closed tab's node may still be in the graph.
        self.image_viewers.retain(|program_id, _| {
            document
                .layout
                .all_tabs()
                .any(|t| t == TabRef::ImageViewer(*program_id))
        });
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use scenarium::{GraphId, NodeId};

    use crate::core::document::internals::DocumentFixture;
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::gui::main_window::MainWindow;
    use crate::gui::pane::graph_ui::internals::CanvasHarness;

    /// A canvas's node caches are swept only when its graph's revision moved: a node removed
    /// behind the edit pipeline's back stays cached, and a removal through it sweeps every node
    /// gone by then. A canvas whose graph no tab shows goes.
    #[test]
    fn the_node_caches_are_swept_only_after_the_revision_moves() {
        let fixture = DocumentFixture::probes(2);
        let (stays, leaves) = (fixture.node(0), fixture.node(1));
        let mut h = CanvasHarness::new(fixture);
        h.prime(2);
        let mut window = MainWindow::default();
        window
            .graph_panes
            .insert(GraphId::Main, mem::take(&mut h.graph_ui));
        let cached = |window: &MainWindow, id: NodeId| {
            window.graph_panes[&GraphId::Main]
                .geometry()
                .caches_node(id)
        };
        window.reconcile(&h.ctx.open);
        assert!(cached(&window, stays) && cached(&window, leaves));

        h.ctx.open.document_mut().remove_node(GraphId::Main, leaves);
        window.reconcile(&h.ctx.open);
        assert!(cached(&window, leaves), "no revision move, no sweep");

        let _facts = h.ctx.open.apply_edit(
            GraphId::Main,
            GraphIntent::RemoveNode { node_id: stays },
            &h.ctx.library,
        );
        window.reconcile(&h.ctx.open);
        assert!(!cached(&window, stays) && !cached(&window, leaves));
    }
}
