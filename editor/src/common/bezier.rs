use egui::epaint::Mesh;
use egui::{Color32, Pos2, Rect, Response, Sense};

use crate::common::connection_bezier::ConnectionBezier;
use crate::common::{pos_changed, scale_changed};
use crate::gui::Gui;
use crate::gui::polyline_mesh::PolylineMesh;

#[derive(Debug, Clone)]
pub struct Bezier {
    mesh: PolylineMesh,
    last_width: f32,
    start: Pos2,
    end: Pos2,
    scale: f32,
    inited: bool,
}

impl Default for Bezier {
    fn default() -> Self {
        Self {
            mesh: PolylineMesh::with_point_capacity(Bezier::DEFAULT_POINTS),
            last_width: 0.0,
            start: Pos2::ZERO,
            end: Pos2::ZERO,
            scale: 1.0,
            inited: false,
        }
    }
}

impl Bezier {
    pub const DEFAULT_POINTS: usize = 25;

    pub fn mesh(&self) -> &Mesh {
        self.mesh.mesh()
    }

    pub fn points(&self) -> &[Pos2] {
        self.mesh.points()
    }

    pub fn update(&mut self, start: Pos2, end: Pos2, scale: f32) -> bool {
        let needs_rebuild = !self.inited
            || pos_changed(self.start, start)
            || pos_changed(self.end, end)
            || scale_changed(self.scale, scale);
        if !needs_rebuild {
            return false;
        }

        self.inited = true;
        self.start = start;
        self.end = end;
        self.scale = scale;

        let points = self.mesh.points_mut();
        if points.len() != Self::DEFAULT_POINTS {
            points.resize(Self::DEFAULT_POINTS, Pos2::ZERO);
        }
        ConnectionBezier::sample(points.as_mut_slice(), start, end, scale);
        true
    }

    pub fn build_mesh(&mut self, start_color: Color32, end_color: Color32, width: f32) {
        assert!(width.is_finite() && width >= 0.0);
        self.last_width = width;
        self.mesh.rebuild(start_color, end_color, width);
    }

    pub fn show(&self, gui: &mut Gui<'_>, sense: Sense, id_salt: impl std::hash::Hash) -> Response {
        let pointer_pos = gui.ui().input(|input| input.pointer.hover_pos());
        let hit = pointer_pos.is_some_and(|pos| self.hit_test(pos));

        let id = gui.ui().make_persistent_id(id_salt);
        let response = if hit {
            let rect = points_bounds(self.mesh.points())
                .map(|rect| rect.expand(self.last_width * 0.5))
                .unwrap_or(Rect::NOTHING);
            gui.ui().interact(rect, id, sense)
        } else {
            gui.ui().interact(Rect::NOTHING, id, Sense::hover())
        };
        self.mesh.render(&gui.painter());
        response
    }

    fn hit_test(&self, pos: Pos2) -> bool {
        let width = self.last_width;
        if width <= 0.0 {
            return false;
        }
        let points = self.mesh.points();
        if points.len() < 2 {
            return false;
        }

        let threshold_sq = width * width * 4.0;
        points
            .windows(2)
            .any(|segment| distance_sq_point_segment(pos, segment[0], segment[1]) <= threshold_sq)
    }
}

fn points_bounds(points: &[Pos2]) -> Option<Rect> {
    if points.is_empty() {
        return None;
    }
    let mut min = points[0];
    let mut max = points[0];
    for point in points.iter().skip(1) {
        min.x = min.x.min(point.x);
        min.y = min.y.min(point.y);
        max.x = max.x.max(point.x);
        max.y = max.y.max(point.y);
    }
    Some(Rect::from_min_max(min, max))
}

fn distance_sq_point_segment(point: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let ap = point - a;
    let ab_len_sq = ab.length_sq();
    if ab_len_sq <= f32::EPSILON {
        return ap.length_sq();
    }
    let t = (ap.dot(ab) / ab_len_sq).clamp(0.0, 1.0);
    let closest = a + ab * t;
    (point - closest).length_sq()
}
