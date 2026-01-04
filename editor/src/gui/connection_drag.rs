use eframe::egui;
use egui::Pos2;

use crate::gui::connection_ui::PortKind;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::PortInfo;
use crate::gui::node_ui;

#[derive(Debug)]
pub struct ConnectionDrag {
    pub start_port: PortInfo,
    pub end_port: Option<PortInfo>,
    pub current_pos: Pos2,
}

impl ConnectionDrag {
    pub fn new(port: PortInfo) -> Self {
        Self {
            current_pos: port.center,
            start_port: port,
            end_port: None,
        }
    }

    pub fn render(&self, ctx: &GraphContext, zoom: f32) {
        let control_offset =
            node_ui::bezier_control_offset(self.start_port.center, self.current_pos, zoom);
        let (start_sign, end_sign) = match self.start_port.port.kind {
            PortKind::Output => (1.0, -1.0),
            PortKind::Input => (-1.0, 1.0),
        };
        let stroke = ctx.style.temp_connection_stroke;
        let shape = egui::epaint::CubicBezierShape::from_points_stroke(
            [
                self.start_port.center,
                self.start_port.center + egui::vec2(control_offset * start_sign, 0.0),
                self.current_pos + egui::vec2(control_offset * end_sign, 0.0),
                self.current_pos,
            ],
            false,
            egui::Color32::TRANSPARENT,
            stroke,
        );
        ctx.painter.add(shape);
    }
}
