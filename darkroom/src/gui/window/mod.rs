pub(crate) mod ctx;
pub(crate) mod menu_bar;
pub(crate) mod status_bar;

use std::collections::HashMap;

use palantir::{Align, Background, Configure, KeyFilter, Panel, Sizing, Ui, VAlign, WidgetId};
use scenarium::{NodeId, OutputTypes};

use crate::core::document::{Document, TabRef};
use crate::core::io::preferences::Preferences;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::prefs::PrefsCommand;
use crate::gui::dock::{DockContext, DockUi};
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::preferences;
use crate::gui::pane::viewer::{self, ImageViewer};
use crate::gui::relayout::Relayout;
use crate::gui::requests::Requests;
use crate::gui::window::ctx::WindowCtx;

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
    dock: DockUi,
    /// The allocation behind the [`GraphCtx`]s below, and nothing more: each
    /// composition refills it, so it carries no state across frames and is a
    /// field only so a refresh reuses its capacity rather than building a map
    /// per pass.
    output_types: OutputTypes,
}

impl MainWindow {
    /// Navigation scan: surface tab activate/close/drag-drop from *last*
    /// frame's chip responses. Runs at the top of the frame so a switch
    /// applies before the record — the switched-to tab records in Pass A and
    /// its connections draw in Pass B, no first-frame gap.
    ///
    /// The dock is the whole of it. The panes' own input reads are the
    /// prepass's, over the arrangement this phase's drain settles.
    pub(crate) fn scan_navigation(&mut self, ui: &mut Ui, cx: WindowCtx<'_>, out: &mut Requests) {
        self.dock.scan(ui, cx.document(), out);
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
                    request_relayout |= graph_ui.prepass(ui, GraphCtx::new(cx, output_types), out);
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
        let doc = cx.document();
        // The menu bar rides its own chrome band; the dock fills the
        // space between it and the status bar.
        let chrome = app.theme().colors.chrome_fill;
        let MainWindow {
            graph_ui,
            image_viewers,
            dock,
            output_types,
        } = self;
        let dock_cx = DockContext {
            open: cx.open(),
            theme: app.theme(),
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
                        // The context carries everything the canvas reads —
                        // theme and run included, through the `cx` it is
                        // composed from.
                        graph_ui.draw(ui, GraphCtx::new(cx, output_types), out);
                    }
                    TabRef::Preferences => {
                        preferences::show(ui, app.theme(), prefs, out);
                    }
                    TabRef::ImageViewer(node_id) => {
                        // Resolved here rather than for every viewer tab up
                        // front: only the pane being drawn needs a title, and
                        // `node_label` is a graph lookup and a `String` — less
                        // than the map that used to front it cost to build.
                        let title = viewer::node_label(doc, node_id);
                        let previews = &app.run_state().previews;
                        let viewer = image_viewers
                            .entry(node_id)
                            .or_insert_with(|| ImageViewer::new(node_id));
                        // Viewer-toolbar edits ride the same in-place
                        // prefs path as the Preferences tab.
                        if viewer.show(ui, app.theme(), &mut prefs.viewer, &title, previews) {
                            out.push_app(AppCommand::Prefs(PrefsCommand::Changed));
                        }
                    }
                });
                // Bottom chrome: the cache-memory readout, below the panes.
                status_bar::show(ui, app);
            });
    }

    /// Release everything this window caches for a subject the document has
    /// stopped holding: the canvas's `NodeId`-keyed tables (see
    /// [`GraphUI::retain_nodes`]) and the per-tab viewer state.
    ///
    /// Driven from `App::update`, beside the preview store's
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
