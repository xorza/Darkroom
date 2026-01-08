use eframe::egui;
use egui::Pos2;

use crate::gui::{graph_ctx::GraphContext, polyline_mesh::PolylineMesh};

const MIN_POINT_DISTANCE: f32 = 4.0;
const MAX_BREAKER_LENGTH: f32 = 900.0;

#[derive(Debug)]
pub struct ConnectionBreaker {
    last_point: Option<Pos2>,
    mesh: PolylineMesh,
}

impl Default for ConnectionBreaker {
    fn default() -> Self {
        Self {
            last_point: None,
            mesh: PolylineMesh::with_point_capacity(max_capacity()),
        }
    }
}

impl ConnectionBreaker {
    pub fn reset(&mut self) {
        self.mesh.points_mut().clear();
        self.last_point = None;
    }

    pub fn start(&mut self, point: Pos2) {
        self.reset();
        self.last_point = Some(point);
        self.mesh.points_mut().push(point);
    }

    pub fn segments(&self) -> impl Iterator<Item = (Pos2, Pos2)> + '_ {
        self.mesh.points().windows(2).map(|pair| (pair[0], pair[1]))
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
        self.mesh.points_mut().push(clamped);
        self.last_point = Some(clamped);
    }

    pub fn is_empty(&self) -> bool {
        self.mesh.points().is_empty()
    }

    pub fn render(&mut self, ctx: &GraphContext) {
        if self.mesh.points().len() < 2 {
            return;
        }

        let pixels_per_point = ctx.ui.ctx().pixels_per_point();
        let feather = 1.0 / pixels_per_point;
        self.mesh.rebuild(
            ctx.style.connections.breaker_stroke.color,
            ctx.style.connections.breaker_stroke.color,
            ctx.style.connections.breaker_stroke.width,
            feather,
        );

        self.mesh.render(&ctx.painter);
    }

    fn path_length(&self) -> f32 {
        self.mesh
            .points()
            .windows(2)
            .map(|pair| pair[0].distance(pair[1]))
            .sum()
    }
}

fn max_capacity() -> usize {
    (MAX_BREAKER_LENGTH / MIN_POINT_DISTANCE).ceil() as usize
}
