use std::collections::HashMap;
use std::mem::take;

use aperture::{Align, Background, Configure, Panel, Sizing, Ui, VAlign};

use crate::core::document::{Document, PortRef, TabRef};
use crate::core::edit::intent::Intent;
use crate::core::io::preferences::Preferences;
use crate::gui::UiAction;
use crate::gui::app::AppContext;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::prefs::PrefsCommand;
use crate::gui::canvas::GraphUI;
use crate::gui::dock::DockUi;
use crate::gui::graph_toolbar;
use crate::gui::image_viewer::ImageViewer;
use crate::gui::menu_bar;
use crate::gui::node::emit_subgraph_opens;
use crate::gui::preferences_view;
use crate::gui::scene::Scene;
use crate::gui::status_bar;

/// Top of darkroom's UI tree: the chrome (menu bar, status bar) around
/// the dock, plus the per-view state the dock's panes render into. The
/// pane *machinery* — strips, splits, drag-docking — is
/// [`DockUi`](crate::gui::dock::DockUi)'s; this file only says what
/// each tab kind looks like (the `content` closure in [`Self::frame`]).
/// Adding a new pane *kind* is a new arm there.
#[derive(Debug)]
pub(crate) struct MainWindow {
    pub(crate) graph_ui: GraphUI,
    /// One full-resolution image-viewer pane per open viewer tab
    /// ([`TabRef::ImageViewer`]), keyed by the port it shows. Fed by
    /// `Editor` (preview clicks + after-run refreshes), which also prunes
    /// entries whose tab closed — dropping one frees its texture.
    pub(crate) image_viewers: HashMap<PortRef, ImageViewer>,
    dock: DockUi,
    first_frame: bool,
}

impl MainWindow {
    /// Navigation scan: surface tab activate/close/drag-drop and
    /// subgraph-open requests from *last* frame's responses (`scene` is
    /// the last-rendered graph, which is what carried the clicked
    /// chips). `App` runs this at the top of the frame so a switch
    /// applies before the record — the switched-to graph records in
    /// Pass A and its connections draw in Pass B, no first-frame gap.
    pub(crate) fn scan_navigation(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        scene: &Scene,
        actions: &mut Vec<UiAction>,
    ) {
        self.dock.scan(ui, doc, actions);
        emit_subgraph_opens(ui, scene, actions);
    }

    /// Edit-phase prepass: input-derived graph mutations for the
    /// already-settled active graph.
    pub(crate) fn prepass(&mut self, ui: &mut Ui, scene: &Scene, out: &mut Vec<Intent>) {
        self.graph_ui.prepass(ui, scene, out);
    }

    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        prefs: &mut Preferences,
        doc: &Document,
        out: &mut Vec<Intent>,
    ) -> Option<AppCommand> {
        let mut command = None;
        // The menu bar rides its own chrome band; the dock fills the
        // space between it and the status bar.
        let chrome = ctx.theme.colors.chrome_fill;
        let MainWindow {
            graph_ui,
            image_viewers,
            dock,
            ..
        } = self;
        Panel::vstack()
            .auto_id()
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                Panel::hstack()
                    .id_salt("chrome_row")
                    .size((Sizing::FILL, Sizing::Hug))
                    .child_align(Align::v(VAlign::Bottom))
                    .background(Background::fill(chrome))
                    .show(ui, |ui| {
                        command = menu_bar::show(ui);
                    });
                dock.render(ui, ctx.theme, doc, out, |ui, tab, out| match tab {
                    TabRef::Graph(_) => {
                        // Overlay the run/cancel toggle on the canvas's
                        // top-left corner; it hit-tests above the canvas,
                        // so a click on it never starts a pan. Graph tabs
                        // live only in the primary group, so the single
                        // canvas scope can't be recorded twice.
                        Panel::zstack()
                            .id_salt("graph_overlay")
                            .size((Sizing::FILL, Sizing::FILL))
                            .show(ui, |ui| {
                                graph_ui.frame(ui, ctx, scene, out, &mut command);
                                if let Some(c) =
                                    graph_toolbar::show(ui, ctx, scene, &graph_ui.geometry, out)
                                {
                                    command = Some(c);
                                }
                            });
                    }
                    TabRef::Preferences => {
                        if let Some(c) = preferences_view::show(ui, ctx.theme, prefs) {
                            command = Some(c);
                        }
                    }
                    TabRef::ImageViewer(port) => {
                        // Recording this pane IS its value request — only a
                        // visible viewer fetches (a background tab isn't shown).
                        ctx.value_requests.watch(port.node_id);
                        let viewer = image_viewers
                            .entry(port)
                            .or_insert_with(|| ImageViewer::new(port));
                        // Viewer-toolbar edits ride the same in-place
                        // prefs path as the Preferences tab.
                        if viewer.show(ui, ctx.theme, &mut prefs.viewer) {
                            command = Some(AppCommand::Prefs(PrefsCommand::Changed));
                        }
                    }
                });
                // Bottom chrome: the cache-memory readout, below the panes.
                status_bar::show(ui, ctx);
            });

        if take(&mut self.first_frame) {
            ui.request_relayout();
        }
        command
    }

    /// Drop transient input bookkeeping (drag anchors, in-flight
    /// connection) when the active tab changes so a gesture started on
    /// one graph can't bleed into another. Keeps `CanvasGeometry`'s offset
    /// cache so the newly-shown graph's connections render immediately.
    pub(crate) fn reset_transient(&mut self) {
        self.graph_ui.clear_gestures();
    }

    /// The viewer pane for `port`, created empty on first access — a
    /// restored/undo-reopened tab has no state yet; it shows the hint
    /// until the after-run refresh fills it.
    pub(crate) fn viewer_mut(&mut self, port: PortRef) -> &mut ImageViewer {
        self.image_viewers
            .entry(port)
            .or_insert_with(|| ImageViewer::new(port))
    }
}

impl Default for MainWindow {
    fn default() -> Self {
        Self {
            graph_ui: GraphUI::default(),
            image_viewers: HashMap::new(),
            dock: DockUi::default(),
            first_frame: true,
        }
    }
}
