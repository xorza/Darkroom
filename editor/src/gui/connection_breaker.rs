use eframe::egui;
use egui::Pos2;

use crate::gui::graph_ctx::GraphContext;

const MIN_POINT_DISTANCE: f32 = 2.0;
const MAX_BREAKER_LENGTH: f32 = 900.0;

#[derive(Debug, Default)]
pub struct ConnectionBreaker {
    segments: Vec<(Pos2, Pos2)>,
    last_point: Option<Pos2>,
}

impl ConnectionBreaker {
    pub fn reset(&mut self) {
        self.segments.clear();
        self.last_point = None;
    }

    pub fn start(&mut self, point: Pos2) {
        self.segments.clear();
        self.last_point = Some(point);
    }

    pub fn segments(&self) -> &[(Pos2, Pos2)] {
        &self.segments
    }

    pub fn add_point(&mut self, point: Pos2) {
        let Some(last_pos) = self.last_point else {
            self.last_point = Some(point);
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
        self.segments.push((last_pos, clamped));
        self.last_point = Some(clamped);
    }

    pub fn render(&self, ctx: &GraphContext) {
        if self.segments.is_empty() {
            return;
        }

        debug_assert!(
            self.segments.windows(2).all(|pair| pair[0].1 == pair[1].0),
            "breaker segments must be contiguous"
        );

        let mut points: Vec<Pos2> = Vec::with_capacity(self.segments.len() + 1);
        points.push(self.segments[0].0);
        points.extend(self.segments.iter().map(|(_, end)| end));

        ctx.painter.line(points, ctx.style.breaker_stroke);
    }

    fn path_length(&self) -> f32 {
        self.segments
            .iter()
            .map(|(start, end)| start.distance(*end))
            .sum()
    }
}
