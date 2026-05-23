use std::mem::take;

use palantir::{Configure, HostHandle, Panel, Sizing, Ui};

use crate::app::AppContext;
use crate::gui::graph_ui::GraphUI;
use crate::gui::menu_bar;
use crate::gui::menu_bar::FileAction;
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

    pub fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &mut Scene,
        host: Option<&HostHandle>,
        out: &mut Vec<Intent>,
    ) -> Option<FileAction> {
        let mut file_action = None;
        Panel::vstack()
            .auto_id()
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                file_action = menu_bar::show(ui, host);
                self.graph_ui.frame(ui, ctx, scene, out);
            });

        if take(&mut self.first_frame) {
            ui.request_relayout();
        }
        file_action
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
