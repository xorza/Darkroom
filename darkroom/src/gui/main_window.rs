use std::collections::HashMap;

use palantir::{Align, Background, Configure, Panel, Sizing, Ui, VAlign};
use scenarium::OutputPort;

use crate::core::document::{Document, TabRef};
use crate::core::edit::intent::sink::Intents;
use crate::core::io::preferences::Preferences;
use crate::gui::UiAction;
use crate::gui::app::AppContext;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::prefs::PrefsCommand;
use crate::gui::canvas::GraphUI;
use crate::gui::canvas::pin_ui::emit_pin_image_opens;
use crate::gui::dock::{DockContext, DockUi};
use crate::gui::graph_toolbar;
use crate::gui::image_viewer::{self, ImageViewer};
use crate::gui::menu_bar;
use crate::gui::node::prepass::emit_graph_opens;
use crate::gui::preferences_view;
use crate::gui::scene::Scene;
use crate::gui::status_bar;

/// Top of darkroom's UI tree: the chrome (menu bar, status bar) around
/// the dock, plus the per-view state the dock's panes render into. The
/// pane *machinery* — strips, splits, drag-docking — is
/// [`DockUi`](crate::gui::dock::DockUi)'s; this file only says what
/// each tab kind looks like (the `content` closure in [`Self::frame`]).
/// Adding a new pane *kind* is a new arm there.
#[derive(Default, Debug)]
pub(crate) struct MainWindow {
    pub(crate) graph_ui: GraphUI,
    /// One image-viewer navigation state per rendered viewer tab
    /// ([`TabRef::ImageViewer`]), keyed by the port it shows. Textures remain
    /// centralized in the pinned-output store.
    pub(crate) image_viewers: HashMap<OutputPort, ImageViewer>,
    dock: DockUi,
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
        scene: &Scene,
        actions: &mut Vec<UiAction>,
    ) {
        self.dock.scan(ui, doc, actions);
        for graph in scene.graphs() {
            emit_graph_opens(ui, graph, actions);
        }
        emit_pin_image_opens(ui, scene, actions);
    }

    /// Edit-phase prepass: input-derived graph mutations for the
    /// already-settled active graph.
    pub(crate) fn prepass(&mut self, ui: &mut Ui, scene: &Scene, out: &mut Intents) {
        self.graph_ui.prepass(ui, scene, out);
    }

    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        prefs: &mut Preferences,
        doc: &Document,
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
            ..
        } = self;
        // One recursive node search + one `String` per viewer tab for the
        // whole frame. Both readers — the strip chip and the pane header —
        // take their label from here.
        let viewer_labels: HashMap<OutputPort, String> = doc
            .viewer_outputs()
            .map(|port| (port, image_viewer::port_label(doc, port)))
            .collect();
        let dock_cx = DockContext {
            doc,
            theme: ctx.theme,
            viewer_labels: &viewer_labels,
        };
        Panel::vstack()
            .auto_id()
            .size((Sizing::FILL, Sizing::FILL))
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
                    TabRef::Graph(target) => {
                        // A graph tab whose projection is missing means the
                        // pane's graph died this frame; `reconcile_with_graph`
                        // prunes the tab before the next one.
                        let Some(graph) = scene.graph(target) else {
                            return;
                        };
                        // Overlay the run/cancel toggle on the canvas's
                        // top-left corner; it hit-tests above the canvas,
                        // so a click on it never starts a pan. Every id
                        // below is salted by `target`, so two graph panes
                        // side by side never record the same widget twice.
                        Panel::zstack()
                            .id_salt(("graph_overlay", target))
                            .size((Sizing::FILL, Sizing::FILL))
                            .show(ui, |ui| {
                                graph_ui.draw(ui, ctx, graph, out, &mut command);
                                if let Some(c) =
                                    graph_toolbar::show(ui, ctx, graph, &graph_ui.geometry, out)
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
                        let title = viewer_labels
                            .get(&port)
                            .map(String::as_str)
                            .unwrap_or("image");
                        let source = ctx.run_state.pinned_outputs.entries.get(&port);
                        let viewer = image_viewers
                            .entry(port)
                            .or_insert_with(|| ImageViewer::new(port));
                        // Viewer-toolbar edits ride the same in-place
                        // prefs path as the Preferences tab.
                        if viewer.show(ui, ctx.theme, &mut prefs.viewer, title, source) {
                            command = Some(AppCommand::Prefs(PrefsCommand::Changed));
                        }
                    }
                });
                // Bottom chrome: the cache-memory readout, below the panes.
                status_bar::show(ui, ctx);
            });

        command
    }

    /// Drop transient input bookkeeping (drag anchors, in-flight
    /// connection) when the active tab changes so a gesture started on
    /// one graph can't bleed into another. Keeps `CanvasGeometry`'s offset
    /// cache so the newly-shown graph's connections render immediately.
    pub(crate) fn reset_transient(&mut self) {
        self.graph_ui.clear_gestures();
    }
}
