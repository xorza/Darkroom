use eframe::egui;
use egui::Pos2;
use graph::graph::NodeId;

use crate::gui::connection_ui::PortKind;
use crate::gui::graph_layout::{PortInfo, PortRef};
use crate::gui::node_ui;
use crate::gui::render::RenderContext;

#[derive(Debug)]
pub struct ConnectionDrag {
    pub start_port: PortRef,
    pub start_pos: Pos2,
    pub current_pos: Pos2,
}

impl Default for ConnectionDrag {
    fn default() -> Self {
        let placeholder = PortRef {
            node_id: NodeId::nil(),
            idx: 0,
            kind: PortKind::Output,
        };
        Self {
            start_port: placeholder,
            start_pos: Pos2::ZERO,
            current_pos: Pos2::ZERO,
        }
    }
}

impl ConnectionDrag {
    pub fn start(&mut self, port: PortInfo) {
        self.start_port = port.port;
        self.start_pos = port.center;
        self.current_pos = port.center;
    }

    pub fn render(&self, ctx: &RenderContext, zoom: f32) {
        let control_offset = node_ui::bezier_control_offset(self.start_pos, self.current_pos, zoom);
        let (start_sign, end_sign) = match self.start_port.kind {
            PortKind::Output => (1.0, -1.0),
            PortKind::Input => (-1.0, 1.0),
        };
        let stroke = ctx.style.temp_connection_stroke;
        let shape = egui::epaint::CubicBezierShape::from_points_stroke(
            [
                self.start_pos,
                self.start_pos + egui::vec2(control_offset * start_sign, 0.0),
                self.current_pos + egui::vec2(control_offset * end_sign, 0.0),
                self.current_pos,
            ],
            false,
            egui::Color32::TRANSPARENT,
            stroke,
        );
        ctx.painter.add(shape);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
