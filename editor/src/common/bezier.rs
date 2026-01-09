use egui::epaint::Mesh;
use egui::{Color32, Painter, Pos2};

use crate::common::connection_bezier::ConnectionBezier;
use crate::gui::polyline_mesh::PolylineMesh;

#[derive(Debug, Clone)]
pub struct Bezier {
    mesh: PolylineMesh,
}

impl Bezier {
    pub const DEFAULT_POINTS: usize = 25;

    pub fn new() -> Self {
        Self {
            mesh: PolylineMesh::with_point_capacity(Self::DEFAULT_POINTS),
        }
    }

    pub fn mesh(&self) -> &Mesh {
        self.mesh.mesh()
    }

    pub fn points(&self) -> &[Pos2] {
        self.mesh.points()
    }

    pub fn build(&mut self, start: Pos2, end: Pos2, scale: f32) {
        let points = self.mesh.points_mut();
        if points.len() != Self::DEFAULT_POINTS {
            points.resize(Self::DEFAULT_POINTS, Pos2::ZERO);
        }
        ConnectionBezier::sample(points.as_mut_slice(), start, end, scale);
    }

    pub fn rebuild(&mut self, start_color: Color32, end_color: Color32, width: f32) {
        self.mesh.rebuild(start_color, end_color, width);
    }

    pub fn render(&self, painter: &Painter) {
        self.mesh.render(painter);
    }
}
