use std::mem::take;

use palantir::Ui;

use crate::frame_result::FrameResult;
use crate::gui::graph_ui::GraphUI;
use crate::scene::Scene;
use crate::theme::{AppContext, Theme};

/// Top of darkroom's UI tree. Owns every persistent UI scope (right
/// now just `GraphUI`) and the canonical [`Theme`]; adding a new
/// top-level pane is a new field + a new dispatch in `frame`.
#[derive(Debug)]
pub struct MainWindow {
    pub graph_ui: GraphUI,
    pub theme: Theme,
    first_frame: bool,
}

impl MainWindow {
    /// Pre-record pass: each UI subtree fills `out` with the intents
    /// it derives from palantir's current-frame input state (drag
    /// deltas, etc.). `App::frame` drains and applies these before
    /// `Scene::rebuild`, so the record phase sees the latest doc.
    pub fn prepass(&mut self, ui: &Ui, scene: &Scene, out: &mut FrameResult) {
        let ctx = AppContext::new(&self.theme);
        self.graph_ui.prepass(ui, &ctx, scene, out);
    }

    pub fn frame(&mut self, ui: &mut Ui, scene: &mut Scene, out: &mut FrameResult) {
        let ctx = AppContext::new(&self.theme);
        self.graph_ui.frame(ui, &ctx, scene, out);

        if take(&mut self.first_frame) {
            ui.request_relayout();
        }
    }
}

impl Default for MainWindow {
    fn default() -> Self {
        Self {
            graph_ui: GraphUI::default(),
            theme: Theme::default(),
            first_frame: true,
        }
    }
}
