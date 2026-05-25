use std::mem::take;

use palantir::{Background, Configure, HostHandle, Panel, Sizing, Ui};

use crate::app::AppContext;
use crate::gui::UiAction;
use crate::gui::graph_ui::GraphUI;
use crate::gui::menu_bar;
use crate::gui::menu_bar::MenuCommand;
use crate::gui::tab_bar::{self, TabLabel};
use crate::intent::Intent;
use crate::scene::Scene;

/// Top of darkroom's UI tree. Owns every persistent UI scope (right
/// now just `GraphUI`); adding a new top-level pane is a new field
/// + a new dispatch in `frame`.
#[derive(Debug)]
pub struct MainWindow {
    pub graph_ui: GraphUI,
    first_frame: bool,
}

impl MainWindow {
    /// Pre-record pass: each UI subtree fills `out` with the intents
    /// it derives from palantir's current-frame input state (drag
    /// deltas, etc.). `App::frame` drains and applies these before
    /// `Scene::rebuild`, so the record phase sees the latest doc.
    pub fn prepass(&mut self, ui: &mut Ui, scene: &Scene, out: &mut Vec<Intent>) {
        self.graph_ui.prepass(ui, scene, out);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &mut Scene,
        host: Option<&HostHandle>,
        tabs: &[TabLabel],
        active: usize,
        out: &mut Vec<Intent>,
        actions: &mut Vec<UiAction>,
    ) -> Option<MenuCommand> {
        let mut command = None;
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
                tab_bar::show(ui, ctx.theme, tabs, active, actions);
                self.graph_ui.frame(ui, ctx, scene, out, actions);
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
    pub fn reset_transient(&mut self) {
        self.graph_ui.clear_gestures();
    }
}

impl Default for MainWindow {
    fn default() -> Self {
        Self {
            graph_ui: GraphUI::default(),
            first_frame: true,
        }
    }
}
