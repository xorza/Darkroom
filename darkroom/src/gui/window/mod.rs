pub(crate) mod menu_bar;
pub(crate) mod status_bar;

use std::collections::HashMap;

use palantir::{Align, Background, Configure, KeyFilter, Panel, Sizing, Ui, VAlign, WidgetId};
use scenarium::{NodeId, OutputTypes};

use crate::core::document::dock::DockOp;
use crate::core::document::{Document, TabRef};
use crate::core::io::preferences::Preferences;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::prefs::PrefsCommand;
use crate::gui::app::ctx::AppCtx;
use crate::gui::dock::{DockContext, DockUi};
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::graph::frame::hits::Chip;
use crate::gui::pane::preferences;
use crate::gui::pane::viewer::{self, ImageViewer};
use crate::gui::requests::Requests;

/// The application root's [`Configure::input_scope`] anchor. A fixed id
/// rather than an auto one because the scope is the thing darkroom's
/// chord handling resolves against, and an auto id moves with the call
/// site.
fn app_root_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.app_root")
}

/// Top of darkroom's UI tree: the chrome (menu bar, status bar) around
/// the dock, plus the per-view state the dock's panes render into. The
/// pane *machinery* — strips, splits, drag-docking — is
/// [`DockUi`]'s; this file only says what
/// each tab kind looks like (the `content` closure in [`Self::frame`]).
/// Adding a new pane *kind* is a new arm there.
///
/// **Where the graph context is composed.** Each entry point below derives its
/// own [`GraphCtx`] from the frame's [`AppCtx`] and the document it is
/// handed — which is settled by the time the caller reaches it, so the
/// construction point and the call are the same instant. Keeping it here
/// rather than in `Editor` means the editor shell never has to name the canvas
/// subsystem's view type, and everything below this file takes that context
/// alone rather than it *and* the app context it came from.
///
/// That is also why the resolved-output table lives here rather than on
/// `Editor`: it is the context's third input, and
/// [`GraphCtx::new`] resolves it against whichever document the
/// entry point was handed. So each of the three below pays one resolve, over a
/// document settled at that instant — the editor drains queued intents
/// *between* them, and a table built once at frame top would be an edit behind
/// by the record pass.
#[derive(Default, Debug)]
pub(crate) struct MainWindow {
    pub(crate) graph_ui: GraphUI,
    /// One image-viewer navigation state per rendered viewer tab
    /// ([`TabRef::ImageViewer`]), keyed by the port it shows. Textures remain
    /// centralized in the preview store.
    pub(crate) image_viewers: HashMap<NodeId, ImageViewer>,
    dock: DockUi,
    /// The allocation behind the [`GraphCtx`]s below, and nothing more: each
    /// composition refills it, so it carries no state across frames and is a
    /// field only so a refresh reuses its capacity rather than building a map
    /// per pass.
    output_types: OutputTypes,
}

impl MainWindow {
    /// Navigation scan: surface tab activate/close/drag-drop and
    /// graph-open requests from *last* frame's responses (`scene` is
    /// the last-rendered graph, which is what carried the clicked
    /// chips). `App` runs this at the top of the frame so a switch
    /// applies before the record — the switched-to graph records in
    /// Pass A and its connections draw in Pass B, no first-frame gap.
    pub(crate) fn scan_navigation(
        &mut self,
        ui: &mut Ui,
        ctx: AppCtx<'_>,
        doc: &Document,
        out: &mut Requests,
    ) {
        self.dock.scan(ui, doc, out);
        // One sweep of last frame's node responses, before anything reads
        // one: the canvas's own passes read it later in the frame, and the
        // two chip opens below are why it has to happen this early. Runs
        // ahead of the tab dispatch, so the context stays an `Option` — with
        // no graph pane up there is nothing to sweep.
        let MainWindow {
            graph_ui,
            output_types,
            ..
        } = self;
        graph_ui.scan_hits(ui, GraphCtx::new(ctx, doc, output_types));
        let hits = &self.graph_ui.hits;
        if let Some(node) = hits.chip(Chip::PreviewImage) {
            out.push_view(DockOp::OpenTab {
                tab: TabRef::ImageViewer(node),
            });
        }
    }

    /// Edit-phase prepass: input-derived graph mutations for the
    /// already-settled active graph, plus the per-pane visibility reconcile
    /// that has to happen before them.
    ///
    /// Returns whether a pane appeared or vanished this frame — the caller
    /// turns that into a relayout request, since a canvas that has never
    /// recorded has no cached geometry to draw its first frame from. Reported
    /// rather than requested here because this pass has no business deciding
    /// when the frame's accumulated signals are spent.
    pub(crate) fn prepass(
        &mut self,
        ui: &mut Ui,
        ctx: AppCtx<'_>,
        doc: &Document,
        out: &mut Requests,
    ) -> bool {
        let MainWindow {
            graph_ui,
            output_types,
            ..
        } = self;
        // Before the per-tab loop, not inside it: a canvas that just went
        // *away* is not an active tab, and its gestures still have to be
        // dropped. `active_tabs` would never visit it.
        let appeared_or_vanished = graph_ui.sync_visibility(doc);
        for tab in doc.layout.active_tabs() {
            match tab {
                // Reached from `active_tabs`, so a pane is showing the graph
                // by construction — which is what `GraphUI::prepass` asserts.
                TabRef::Graph => graph_ui.prepass(ui, GraphCtx::new(ctx, doc, output_types), out),
                // Neither derives a document mutation from input: preferences
                // edits go through their own widgets, and a viewer only
                // navigates its own texture.
                TabRef::Preferences | TabRef::ImageViewer(_) => {}
            }
        }
        appeared_or_vanished
    }

    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: AppCtx<'_>,
        doc: &Document,
        prefs: &mut Preferences,
        out: &mut Requests,
    ) {
        // The menu bar rides its own chrome band; the dock fills the
        // space between it and the status bar.
        let chrome = ctx.theme().colors.chrome_fill;
        let MainWindow {
            graph_ui,
            image_viewers,
            dock,
            output_types,
        } = self;
        // One recursive node search + one `String` per viewer tab for the
        // whole frame. Both readers — the strip chip and the pane header —
        // take their label from here, rather than each re-running the search.
        //
        // Rebuilt every frame: a label depends on the producing node's name,
        // and nothing signals a rename cheaply enough to cache against. The
        // cost is proportional to *open viewer tabs*, which is normally zero,
        // so it stays off the common path on its own.
        let viewer_labels: HashMap<NodeId, String> = doc
            .viewer_nodes()
            .map(|node_id| (node_id, viewer::node_label(doc, node_id)))
            .collect();
        let dock_cx = DockContext {
            doc,
            theme: ctx.theme(),
            viewer_labels: &viewer_labels,
        };
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
            .input_scope(KeyFilter::all() - KeyFilter::TEXT)
            .show(ui, |ui| {
                Panel::hstack()
                    .id_salt("chrome_row")
                    .size((Sizing::FILL, Sizing::HUG))
                    .child_align(Align::v(VAlign::Bottom))
                    .background(Background::fill(chrome))
                    .show(ui, |ui| {
                        menu_bar::show(ui, out);
                    });
                dock.render(ui, dock_cx, out, |ui, tab, out| match tab {
                    TabRef::Graph => {
                        // Overlay the run/cancel toggle on the canvas's
                        // top-left corner; it hit-tests above the canvas,
                        // so a click on it never starts a pan.
                        Panel::zstack()
                            .id_salt("graph_overlay")
                            .size((Sizing::FILL, Sizing::FILL))
                            .show(ui, |ui| {
                                // The context carries everything the canvas
                                // reads — theme and run included, through the
                                // `ctx` it is composed from.
                                let graph_ctx = GraphCtx::new(ctx, doc, output_types);
                                graph_ui.draw(ui, graph_ctx, out);
                                graph_ui.draw_toolbar(ui, graph_ctx, out);
                            });
                    }
                    TabRef::Preferences => {
                        preferences::show(ui, ctx.theme(), prefs, out);
                    }
                    TabRef::ImageViewer(node_id) => {
                        let title = viewer_labels
                            .get(&node_id)
                            .map(String::as_str)
                            .unwrap_or("image");
                        let source = ctx.run_state().previews.entries.get(&node_id);
                        let viewer = image_viewers
                            .entry(node_id)
                            .or_insert_with(|| ImageViewer::new(node_id));
                        // Viewer-toolbar edits ride the same in-place
                        // prefs path as the Preferences tab.
                        if viewer.show(ui, ctx.theme(), &mut prefs.viewer, title, source) {
                            out.push_app(AppCommand::Prefs(PrefsCommand::Changed));
                        }
                    }
                });
                // Bottom chrome: the cache-memory readout, below the panes.
                status_bar::show(ui, ctx);
            });
    }

    /// Release everything this window caches for a subject the document has
    /// stopped holding: the canvas's `NodeId`-keyed tables (see
    /// [`GraphUI::retain_nodes`]) and the per-tab viewer state.
    ///
    /// Driven from `App::reconcile_derived_state`, beside the preview store's
    /// sweep. Both live here because `MainWindow` owns both, so a new cache
    /// joins them rather than earning its own call site.
    pub(crate) fn reconcile(&mut self, document: &Document) {
        self.graph_ui.retain_nodes(document);
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
