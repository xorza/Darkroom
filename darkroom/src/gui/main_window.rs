use std::mem::take;

use palantir::{Align, Configure, HostHandle, Panel, Sizing, Ui, VAlign};

use crate::app::AppContext;
use crate::gui::graph_ui::GraphUI;
use crate::gui::menu_bar;
use crate::gui::menu_bar::MenuCommand;
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
    ) -> Option<MenuCommand> {
        let mut command = None;
        // ZStack so the menu bar floats *over* the graph rather than
        // reserving a row above it. Record order = paint order: the
        // graph paints first (full-pane backdrop), the menu bar on top.
        // The bar has no background and its triggers are transparent
        // until hovered, so nodes show through behind it. `child_align`
        // top pins the Hug-height bar to the top edge (the FILL graph
        // ignores it).
        Panel::zstack()
            .auto_id()
            .size((Sizing::FILL, Sizing::FILL))
            .child_align(Align::v(VAlign::Top))
            .show(ui, |ui| {
                self.graph_ui.frame(ui, ctx, scene, out);
                command = menu_bar::show(ui, host);
            });

        if take(&mut self.first_frame) {
            ui.request_relayout();
        }
        command
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
