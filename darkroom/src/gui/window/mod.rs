pub(crate) mod dock_panes;
pub(crate) mod menu_bar;
pub(crate) mod status_bar;
pub(crate) mod window_ctx;

use std::collections::HashMap;

use palantir::prelude::*;
use palantir::{DockOperation, DockView, KeyFilter, TabOverflow};
use scenarium::NodeId;

use crate::core::document::TabRef;
use crate::core::document::graph_revision::GraphRevision;
use crate::core::document::open_document::OpenDocument;
use crate::core::edit::relayout::Relayout;
use crate::core::io::preferences::Preferences;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::graph_ctx::output_type_cache::OutputTypeCache;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::viewer::ImageViewer;
use crate::gui::requests::Requests;
use crate::gui::window::dock_panes::DockPanes;
use crate::gui::window::window_ctx::WindowCtx;

/// The application root's [`Configure::input_scope`] anchor. A fixed id
/// rather than an auto one because the scope is the thing darkroom's
/// chord handling resolves against, and an auto id moves with the call
/// site.
fn app_root_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.app_root")
}

/// Smallest a dock pane can be squeezed on its split axis, in logical
/// px.
const MIN_PANE: f32 = 220.0;

/// Top of darkroom's UI tree: the chrome (menu bar, status bar) around
/// the dock, plus the per-view state the dock's panes render into. The
/// pane *machinery* — strips, splits, drag-docking — is
/// [`DockView`]'s, and what each tab kind looks like is
/// [`DockPanes`]'s. Adding a new pane *kind* is a new arm there.
///
/// **Where the graph context is composed.** Each entry point below takes a
/// [`WindowCtx`] — the frame's world *and* the document it is showing, settled
/// by the time the caller reaches it, so the composition point and the call
/// are the same instant — and derives its own [`GraphCtx`] from it. Keeping
/// that here rather than in `Editor` means the editor shell never has to name
/// the canvas subsystem's view type, and everything below this file takes that
/// context alone rather than it *and* the levels it came from.
///
/// That is also why the resolved-output table lives here rather than on
/// `Editor`: it is the canvas context's second input, and [`GraphCtx::new`]
/// resolves it against whichever document the entry point was handed. So each
/// of the three below pays one resolve, over a document settled at that
/// instant — the editor drains queued intents *between* them, and a table
/// built once at frame top would be an edit behind by the record pass.
#[derive(Default, Debug)]
pub(crate) struct MainWindow {
    pub(crate) graph_ui: GraphUI,
    /// One image-viewer navigation state per rendered viewer tab
    /// ([`TabRef::ImageViewer`]), keyed by the port it shows. Textures remain
    /// centralized in the preview store.
    pub(crate) image_viewers: HashMap<NodeId, ImageViewer>,
    /// The dock's op sink for one frame. A field so the two phases that
    /// fill it — the navigation scan and the record — reuse one buffer's
    /// capacity rather than building a `Vec` per frame.
    dock_ops: Vec<DockOperation<TabRef>>,
    /// The open document's resolved output types, kept across frames and
    /// resolved again only after an edit that can retype an output.
    output_types: OutputTypeCache,
    /// The graph revision the canvas's node caches were last swept at. A node
    /// can only go with a step that moves the revision, so a frame at the
    /// same one has nothing to sweep.
    swept_at: Option<GraphRevision>,
}

impl MainWindow {
    /// Navigation scan: surface tab activate/close/drag-drop from *last*
    /// frame's chip responses. Runs at the top of the frame so a switch
    /// applies before the record — the switched-to tab records in Pass A and
    /// its connections draw in Pass B, no first-frame gap.
    ///
    /// The dock is the whole of it. The panes' own input reads are the
    /// prepass's, over the arrangement this phase's drain settles.
    ///
    /// The scan emits into the window's own buffer and the ops are then
    /// handed to the frame's queue, which is where every other darkroom
    /// surface puts one: dock ops travel beside graph edits, stay out of
    /// undo, and are validated before a save.
    pub(crate) fn scan_navigation(&mut self, ui: &mut Ui, cx: WindowCtx<'_>, out: &mut Requests) {
        let ops = &mut self.dock_ops;
        ops.clear();
        DockView::scan(ui, &cx.document().layout, ops);
        for op in ops.drain(..) {
            out.push_view(op);
        }
    }

    /// Edit-phase prepass: input-derived graph mutations for the
    /// already-settled active graph, plus the per-pane visibility reconcile
    /// that has to happen before them.
    ///
    /// Returns whether any pane became visible this frame — the caller turns
    /// that into a relayout request, since a canvas that has never recorded
    /// has no cached geometry to draw its first frame from. A pane that
    /// *vanished* needs no pass and is never visited here at all. Reported
    /// rather than requested because this pass has no business deciding when
    /// the frame's accumulated signals are spent.
    pub(crate) fn prepass(
        &mut self,
        ui: &mut Ui,
        cx: WindowCtx<'_>,
        out: &mut Requests,
    ) -> Relayout {
        let MainWindow {
            graph_ui,
            output_types,
            ..
        } = self;
        let mut request_relayout = Relayout::NotNeeded;
        for tab in cx.document().layout.active_tabs() {
            match tab {
                // Reached from `active_tabs`, so a pane is showing the graph
                // by construction — which is what `GraphUI::prepass` asserts.
                TabRef::Graph => {
                    let types = output_types.refresh(cx.open(), cx.app().shared_library());
                    request_relayout |= graph_ui.prepass(ui, GraphCtx::new(cx, types), out);
                }
                // Neither derives a document mutation from input: preferences
                // edits go through their own widgets, and a viewer only
                // navigates its own texture.
                TabRef::Preferences | TabRef::ImageViewer(_) => {}
            }
        }
        request_relayout
    }

    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        cx: WindowCtx<'_>,
        prefs: &mut Preferences,
        out: &mut Requests,
    ) {
        // The frame's world without the document, for the surfaces below that
        // read one and not the other: the chrome bands take the app context,
        // the panes take the whole thing.
        let app = cx.app();
        // The menu bar rides its own chrome band; the dock fills the
        // space between it and the status bar.
        let chrome = app.theme().colors.chrome_fill;
        let MainWindow {
            graph_ui,
            image_viewers,
            dock_ops,
            output_types,
            swept_at: _,
        } = self;
        Panel::vstack()
            .id(app_root_wid())
            .size((Sizing::FILL, Sizing::FILL))
            // The application's input scope, and the only one darkroom
            // declares — palantir's overlays and text fields bring their
            // own. Everything except `TEXT`: a canvas has no typing, so a
            // focused editor's characters, its Ctrl+Z, its Delete and its
            // Escape all stop here rather than doubling as graph edits,
            // while `ACCEL` (Ctrl+S, Ctrl+R, …) still lands on the app
            // mid-edit.
            .input_scope(KeyFilter::ALL.difference(KeyFilter::TEXT))
            .show(ui, |ui| {
                Panel::hstack()
                    .id_salt("chrome_row")
                    .size((Sizing::FILL, Sizing::HUG))
                    .child_align(Align::v(VAlign::Bottom))
                    .background(Background::fill(chrome))
                    .show(ui, |ui| {
                        menu_bar::show(ui, app.theme(), out);
                    });
                let mut panes = DockPanes {
                    cx,
                    graph_ui,
                    image_viewers,
                    output_types: output_types.refresh(cx.open(), app.shared_library()),
                    prefs,
                    out,
                };
                dock_ops.clear();
                DockView::new(&cx.document().layout, dock_ops)
                    .min_pane(MIN_PANE)
                    .overflow(TabOverflow::Menu)
                    .show(ui, &mut panes);
                // Ratio drags and split-menu picks, onto the frame's own
                // queue — the same route the navigation scan's ops take.
                for op in dock_ops.drain(..) {
                    out.push_view(op);
                }
                // Bottom chrome: the cache-memory readout, below the panes.
                status_bar::show(ui, app);
            });
    }

    /// Release everything this window caches for a subject the document has
    /// stopped holding: the canvas's `NodeId`-keyed tables (see
    /// [`GraphUI::retain_nodes`]), swept only when the graph's revision moved,
    /// and the per-tab viewer state.
    ///
    /// Driven from `App::update`, beside the preview store's
    /// sweep. Both live here because `MainWindow` owns both, so a new cache
    /// joins them rather than earning its own call site.
    pub(crate) fn reconcile(&mut self, open: &OpenDocument) {
        let document = &open.document;
        let revision = open.graph_revision();
        if self.swept_at != Some(revision) {
            self.graph_ui.retain_nodes(document);
            self.swept_at = Some(revision);
        }
        // Keyed by node, but scoped to its *tab*: a viewer's framing dies when
        // the tab closes, not when the node does — and a closed tab's node may
        // well still be in the graph.
        self.image_viewers.retain(|node_id, _| {
            document
                .layout
                .all_tabs()
                .any(|t| t == TabRef::ImageViewer(*node_id))
        });
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use scenarium::NodeId;

    use crate::core::document::internals::DocFixture;
    use crate::core::edit::graph_intent::GraphIntent;
    use crate::gui::pane::graph::internals::CanvasHarness;
    use crate::gui::window::MainWindow;

    /// The canvas's node caches are swept only when the graph's revision
    /// moved. A node removed behind the edit pipeline's back — which nothing
    /// in production does — stays cached, which is what shows a frame at the
    /// same revision sweeps nothing; a removal through the pipeline sweeps
    /// every node gone by then.
    #[test]
    fn the_node_caches_are_swept_only_after_the_revision_moves() {
        let fixture = DocFixture::probes(2);
        let (stays, leaves) = (fixture.node(0), fixture.node(1));
        let mut h = CanvasHarness::new(fixture);
        h.prime(2);
        let mut window = MainWindow {
            graph_ui: mem::take(&mut h.graph_ui),
            ..MainWindow::default()
        };
        let cached = |window: &MainWindow, id: NodeId| window.graph_ui.geometry().caches_node(id);
        window.reconcile(&h.ctx.open);
        assert!(cached(&window, stays) && cached(&window, leaves));

        h.ctx.open.document.remove_node(leaves);
        window.reconcile(&h.ctx.open);
        assert!(cached(&window, leaves), "no revision move, no sweep");

        let _relayout = h
            .ctx
            .open
            .apply_edit(GraphIntent::RemoveNode { node_id: stays }, &h.ctx.library);
        window.reconcile(&h.ctx.open);
        assert!(!cached(&window, stays) && !cached(&window, leaves));
    }
}
