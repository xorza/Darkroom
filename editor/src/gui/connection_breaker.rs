use eframe::egui;
use egui::Pos2;

use crate::gui::graph_ctx::GraphContext;

const MIN_POINT_DISTANCE: f32 = 2.0;
const MAX_BREAKER_LENGTH: f32 = 900.0;

#[derive(Debug, Default)]
pub struct ConnectionBreaker {
    pub points: Vec<Pos2>,
}

impl ConnectionBreaker {
    pub fn reset(&mut self) {
        self.points.clear();
    }

    pub fn add_point(&mut self, point: Pos2) {
        let Some(last_pos) = self.points.last().copied() else {
            self.points.push(point);
            return;
        };
        if last_pos.distance(point) <= MIN_POINT_DISTANCE {
            return;
        }

        let remaining = MAX_BREAKER_LENGTH - self.path_length();
        if remaining <= 0.0 {
            return;
        }

        let segment_len = last_pos.distance(point);
        if segment_len <= 0.0 {
            return;
        }

        let clamped = if segment_len <= remaining {
            point
        } else {
            let t = remaining / segment_len;
            Pos2::new(
                last_pos.x + (point.x - last_pos.x) * t,
                last_pos.y + (point.y - last_pos.y) * t,
            )
        };
        self.points.push(clamped);
    }

    pub fn render(&self, ctx: &GraphContext) {
        if self.points.len() > 1 {
            ctx.painter.add(egui::Shape::line(
                self.points.clone(),
                ctx.style.breaker_stroke,
            ));
        }
    }

    fn path_length(&self) -> f32 {
        self.points
            .windows(2)
            .map(|pair| pair[0].distance(pair[1]))
            .sum()
    }
}
