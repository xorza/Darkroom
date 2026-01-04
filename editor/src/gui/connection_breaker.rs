use eframe::egui;
use egui::Pos2;

use crate::gui::graph_ctx::GraphContext;

#[derive(Debug, Default)]
pub struct ConnectionBreaker {
    pub points: Vec<Pos2>,
}

impl ConnectionBreaker {
    pub fn reset(&mut self) {
        self.points.clear();
    }

    pub fn render(&self, ctx: &GraphContext) {
        if self.points.len() > 1 {
            ctx.painter.add(egui::Shape::line(
                self.points.clone(),
                ctx.style.breaker_stroke,
            ));
        }
    }
}
