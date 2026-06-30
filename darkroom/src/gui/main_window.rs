use std::mem::take;

use palantir::{Background, Configure, Panel, Sizing, Ui};

use crate::core::document::{Document, GraphRef, TabRef};
use crate::core::edit::intent::Intent;
use crate::gui::HostHandle;
use crate::gui::UiAction;
use crate::gui::app::AppCommand;
use crate::gui::app::AppContext;
use crate::gui::canvas::GraphUI;
use crate::gui::config_view;
use crate::gui::graph_toolbar;
use crate::gui::menu_bar;
use crate::gui::node::emit_subgraph_opens;
use crate::gui::scene::Scene;
use crate::gui::tab_bar::{self, TabLabel};

/// Top of darkroom's UI tree. Owns every persistent UI scope (right
/// now just `GraphUI`); adding a new top-level pane is a new field
/// + a new dispatch in `frame`.
#[derive(Debug)]
pub(crate) struct MainWindow {
    pub(crate) graph_ui: GraphUI,
    first_frame: bool,
}

impl MainWindow {
    /// Navigation scan: surface tab activate/close and subgraph-open
    /// requests from *last* frame's responses (`scene` is the
    /// last-rendered graph, which is what carried the clicked chips).
    /// `App` runs this at the top of the frame so a switch applies before
    /// the record — the switched-to graph records in Pass A and its
    /// connections draw in Pass B, no first-frame gap.
    pub(crate) fn scan_navigation(
        &self,
        ui: &Ui,
        doc: &Document,
        scene: &Scene,
        actions: &mut Vec<UiAction>,
    ) {
        tab_bar::emit_tab_actions(ui, &doc.tabs, actions);
        emit_subgraph_opens(ui, scene, actions);
    }

    /// Edit-phase prepass: input-derived graph mutations for the
    /// already-settled active graph.
    pub(crate) fn prepass(&mut self, ui: &mut Ui, scene: &Scene, out: &mut Vec<Intent>) {
        self.graph_ui.prepass(ui, scene, out);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &Scene,
        host: Option<&HostHandle>,
        doc: &Document,
        out: &mut Vec<Intent>,
    ) -> Option<AppCommand> {
        let mut command = None;
        let tabs = tab_labels(doc);
        // Top-to-bottom: menu bar, then the tab strip, then the graph.
        // Menu bar + tabs share the `chrome_fill` color so they read as
        // one top bar; the graph pane (canvas_bg) fills the rest, and the
        // active tab punches through to that same color so it looks
        // continuous with the canvas below it.
        let chrome = ctx.theme.chrome_fill;
        Panel::vstack()
            .auto_id()
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                Panel::hstack()
                    .id_salt("menu_chrome")
                    .size((Sizing::FILL, Sizing::Hug))
                    .background(Background {
                        fill: chrome.into(),
                        ..Default::default()
                    })
                    .show(ui, |ui| {
                        command = menu_bar::show(ui, host);
                    });
                tab_bar::show(ui, ctx.theme, &tabs, doc.active, out);
                // The content pane below the strip is the active tab's view:
                // the graph canvas for a graph tab, or the config window for
                // the non-graph Config tab.
                match doc.active_tab() {
                    TabRef::Graph(_) => {
                        // Overlay the run/cancel toggle on the canvas's
                        // top-left corner; it hit-tests above the canvas, so a
                        // click on it never starts a pan.
                        Panel::zstack()
                            .id_salt("graph_overlay")
                            .size((Sizing::FILL, Sizing::FILL))
                            .show(ui, |ui| {
                                self.graph_ui.frame(ui, ctx, scene, out, &mut command);
                                if let Some(c) = graph_toolbar::show(ui, ctx) {
                                    command = Some(c);
                                }
                            });
                    }
                    TabRef::Config => {
                        if let Some(c) = config_view::show(ui, ctx) {
                            command = Some(c);
                        }
                    }
                }
            });

        if take(&mut self.first_frame) {
            ui.request_relayout();
        }
        command
    }

    /// Drop transient input bookkeeping (drag anchors, in-flight
    /// connection) when the active tab changes so a gesture started on
    /// one graph can't bleed into another. Keeps `PortFrame`'s offset
    /// cache so the newly-shown graph's connections render immediately.
    pub(crate) fn reset_transient(&mut self) {
        self.graph_ui.clear_gestures();
    }
}

/// Project the document's open-tab list into the strip's per-tab
/// labels. Lives here (not in `tab_bar`) so the strip stays
/// document-agnostic — same split as `Scene` for the canvas.
fn tab_labels(doc: &Document) -> Vec<TabLabel> {
    doc.tabs
        .iter()
        .map(|t| match t {
            TabRef::Graph(GraphRef::Main) => TabLabel {
                text: "main".into(),
                subgraph_id: None,
                closable: false,
            },
            TabRef::Graph(GraphRef::Local(id)) => {
                let name = doc
                    .graph
                    .subgraphs
                    .by_key(id)
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| "subgraph".to_string());
                TabLabel {
                    text: name.into(),
                    subgraph_id: Some(*id),
                    closable: true,
                }
            }
            TabRef::Config => TabLabel {
                text: "config".into(),
                subgraph_id: None,
                closable: true,
            },
        })
        .collect()
}

impl Default for MainWindow {
    fn default() -> Self {
        Self {
            graph_ui: GraphUI::default(),
            first_frame: true,
        }
    }
}
