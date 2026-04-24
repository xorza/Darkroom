use eframe::egui;
use egui::ViewportCommand;

use crate::ui_host::UiHost;

#[derive(Debug)]
pub struct EguiUiHost {
    ctx: egui::Context,
}

impl EguiUiHost {
    pub fn new(ctx: &egui::Context) -> Self {
        Self { ctx: ctx.clone() }
    }
}

impl UiHost for EguiUiHost {
    fn request_redraw(&self) {
        self.ctx.request_repaint();
    }

    fn close_app(&self) {
        self.ctx.send_viewport_cmd(ViewportCommand::Close);
    }
}
