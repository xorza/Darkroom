use eframe::egui;
use egui::Pos2;

use crate::gui::{Gui, polyline_mesh::PolylineMesh};

const MIN_POINT_DISTANCE: f32 = 4.0;
const MAX_BREAKER_LENGTH: f32 = 900.0;

#[derive(Debug)]
pub struct ConnectionBreaker {
    mesh: PolylineMesh,
    built_len: usize,
    reset: bool,
}

impl Default for ConnectionBreaker {
    fn default() -> Self {
        Self {
            mesh: PolylineMesh::with_point_capacity(max_capacity()),
            built_len: 0,
            reset: false,
        }
    }
}

impl ConnectionBreaker {
    pub fn reset(&mut self) {
        self.mesh.points_mut().clear();
        self.built_len = 0;
        self.reset = true;
    }

    pub fn start(&mut self, point: Pos2) {
        self.reset();
        self.mesh.points_mut().push(point);
    }

    pub fn segments(&self) -> impl Iterator<Item = (Pos2, Pos2)> + '_ {
        self.mesh.points().windows(2).map(|pair| (pair[0], pair[1]))
    }

    pub fn add_point(&mut self, point: Pos2) {
        let last_pos = self
            .mesh
            .points()
            .last()
            .copied()
            .expect("ConnectionBreaker should be started before adding points");
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
    }

    pub fn show(&mut self, gui: &mut Gui<'_>) {
        let point_len = self.mesh.points().len();
        if self.reset || point_len < self.built_len {
            self.mesh.clear_mesh();
            self.reset = false;
        }

        let color = gui.style.connections.breaker_stroke.color;
        if point_len > self.built_len {
            let start_segment = self.built_len.saturating_sub(1);
            self.mesh.append_segments_from_points(
                start_segment,
                color,
                color,
                gui.style.connections.breaker_stroke.width,
            );
            self.built_len = point_len;
        }

        self.mesh.render(&gui.painter());
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
