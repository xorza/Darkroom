use std::mem::take;

use palantir::Ui;

use crate::frame_result::FrameResult;
use crate::gui::graph_ui::GraphUI;
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
    pub fn frame(&mut self, ui: &mut Ui, scene: &Scene, out: &mut FrameResult) {
        self.graph_ui.frame(ui, scene, out);

        if take(&mut self.first_frame) {
            ui.request_relayout();
        }
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
