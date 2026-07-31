use std::collections::HashMap;

use palantir::{Align, Background, Configure, KeyFilter, Panel, Sizing, Ui, VAlign, WidgetId};
use scenarium::{Library, NodeId, OutputTypes};

use crate::core::document::{Document, TabRef};
use crate::core::edit::intent::sink::Intents;
use crate::core::io::preferences::Preferences;
use crate::gui::UiAction;
use crate::gui::app::AppContext;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::prefs::PrefsCommand;
use crate::gui::canvas::GraphUI;
use crate::gui::canvas::hits::Chip;
use crate::gui::dock::{DockContext, DockUi};
use crate::gui::graph_scope::GraphScope;
use crate::gui::image_viewer::{self, ImageViewer};
use crate::gui::menu_bar;
use crate::gui::preferences_view;
use crate::gui::run_state::RunState;
use crate::gui::status_bar;

/// The application root's [`Configure::input_scope`] anchor. A fixed id
/// rather than an auto one because the scope is the thing darkroom's
/// chord handling resolves against, and an auto id moves with the call
/// site.
fn app_root_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.app_root")
}

/// Offer `produce`'s command to the frame's single [`AppCommand`] slot, first
/// claim winning.
///
/// Exactly one command leaves a frame, and this is the only thing that decides
/// which: the menu bar records first, then every visible pane in dock order.
/// Without it a later pane silently overwrote the menu-bar pick, or one pane's
/// overwrote its neighbour's — so every surface that can raise a command goes
/// through here rather than reaching for the slot itself.
///
/// `produce` still runs when the slot is taken: these surfaces have to record
/// every frame regardless, and only the command they'd have contributed is
/// dropped. In practice nothing is: every source here reads a pointer click,
/// and one pointer produces one click. (The keyboard's own commands are a
/// separate source, merged by `Editor::frame`.)
fn claim(slot: &mut Option<AppCommand>, produce: impl FnOnce() -> Option<AppCommand>) {
    let produced = produce();
    if slot.is_none() {
        *slot = produced;
    }
}

/// Top of darkroom's UI tree: the chrome (menu bar, status bar) around
/// the dock, plus the per-view state the dock's panes render into. The
/// pane *machinery* — strips, splits, drag-docking — is
/// [`DockUi`]'s; this file only says what
/// each tab kind looks like (the `content` closure in [`Self::frame`]).
/// Adding a new pane *kind* is a new arm there.
///
/// **Where the graph scope is composed.** Each entry point below builds its
/// own [`GraphScope`] from the document it is handed — which is settled by
/// the time the caller reaches it, so the construction point and the call
/// are the same instant. Keeping it here rather than in `Editor` means the
/// editor shell never has to name the canvas subsystem's view type.
///
/// That is also why the resolved-output table lives here rather than on
/// `Editor`: it is the scope's fourth input, and
/// [`GraphScope::for_document`] resolves it against whichever document the
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
    /// The allocation behind the [`GraphScope`]s below, and nothing more: each
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
        doc: &Document,
        library: &Library,
        run_state: &RunState,
        actions: &mut Vec<UiAction>,
    ) {
        self.dock.scan(ui, doc, actions);
        // One sweep of last frame's node responses, before anything reads
        // one: the canvas's own passes read it later in the frame, and the
        // two chip opens below are why it has to happen this early. Runs
        // ahead of the tab dispatch, so the scope stays an `Option` — with
        // no graph pane up there is nothing to sweep.
        let MainWindow {
            graph_ui,
            output_types,
            ..
        } = self;
        graph_ui.scan_hits(
            ui,
            GraphScope::for_document(doc, library, run_state, output_types),
        );
        let hits = &self.graph_ui.hits;
        if let Some(node) = hits.chip(Chip::PreviewImage) {
            actions.push(UiAction::OpenImageViewer(node));
        }
    }

    /// Edit-phase prepass: input-derived graph mutations for the
    /// already-settled active graph.
    pub(crate) fn prepass(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        library: &Library,
        run_state: &RunState,
        out: &mut Intents,
    ) {
        let MainWindow {
            graph_ui,
            output_types,
            ..
        } = self;
        for tab in doc.layout.active_tabs() {
            match tab {
                // `Document::shows_graph` — which the scope gates on — is
                // this same predicate over the same groups, so matching the
                // arm *is* the proof that it resolves.
                TabRef::Graph => graph_ui.prepass(
                    ui,
                    GraphScope::for_document(doc, library, run_state, output_types)
                        .expect("a Graph tab is active, so the scope resolves"),
                    out,
                ),
                // Neither derives a document mutation from input: preferences
                // edits go through their own widgets, and a viewer only
                // navigates its own texture.
                TabRef::Preferences | TabRef::ImageViewer(_) => {}
            }
        }
    }

    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        doc: &Document,
        prefs: &mut Preferences,
        out: &mut Intents,
    ) -> Option<AppCommand> {
        let mut command = None;
        // The menu bar rides its own chrome band; the dock fills the
        // space between it and the status bar.
        let chrome = ctx.theme.colors.chrome_fill;
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
            .map(|node_id| (node_id, image_viewer::node_label(doc, node_id)))
            .collect();
        let dock_cx = DockContext {
            doc,
            theme: ctx.theme,
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
                        command = menu_bar::show(ui);
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
                                // `ctx` carries both halves this needs beside
                                // the document; see `Self::prepass` for why
                                // matching the arm proves it resolves.
                                let graph_scope = GraphScope::for_document(
                                    doc,
                                    ctx.library,
                                    ctx.run_state,
                                    output_types,
                                )
                                .expect("a Graph tab is active, so the scope resolves");
                                claim(&mut command, || graph_ui.draw(ui, ctx, graph_scope, out));
                                claim(&mut command, || {
                                    graph_ui.draw_toolbar(ui, ctx, graph_scope, out)
                                });
                            });
                    }
                    TabRef::Preferences => {
                        claim(&mut command, || {
                            preferences_view::show(ui, ctx.theme, prefs)
                        });
                    }
                    TabRef::ImageViewer(node_id) => {
                        let title = viewer_labels
                            .get(&node_id)
                            .map(String::as_str)
                            .unwrap_or("image");
                        let source = ctx.run_state.previews.entries.get(&node_id);
                        let viewer = image_viewers
                            .entry(node_id)
                            .or_insert_with(|| ImageViewer::new(node_id));
                        // Viewer-toolbar edits ride the same in-place
                        // prefs path as the Preferences tab.
                        claim(&mut command, || {
                            viewer
                                .show(ui, ctx.theme, &mut prefs.viewer, title, source)
                                .then_some(AppCommand::Prefs(PrefsCommand::Changed))
                        });
                    }
                });
                // Bottom chrome: the cache-memory readout, below the panes.
                status_bar::show(ui, ctx);
            });

        command
    }

    /// Release the canvas's cached geometry for nodes the document has
    /// stopped holding. The counterpart to the preview store's reconcile:
    /// both are `NodeId`-keyed caches that outlive the scene on purpose, so
    /// both need the document to tell them when an entry's subject is gone for
    /// good, and both run once a frame.
    pub(crate) fn reconcile(&mut self, document: &Document) {
        self.graph_ui
            .retain_nodes(|node_id| document.holds_node(node_id));
    }
}
