use egui::epaint::Mesh;
use egui::{Color32, Pos2, Rect, Response, Sense};

use crate::common::{bezier_helper, pos_changed_p2, scale_changed};
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::polyline_mesh::PolylineMesh;
use crate::gui::{Gui, style};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConnectionBezierStyle {
    pub(crate) start_color: Color32,
    pub(crate) end_color: Color32,
    pub(crate) stroke_width: f32,
}

#[derive(Debug, Clone)]
pub struct ConnectionBezier {
    polyline: PolylineMesh,
    stroke_width: f32,
    start: Pos2,
    end: Pos2,
    scale: f32,
    inited: bool,
    hovered: bool,
    broke: bool,
    points_dirty: bool,
    feather: f32,
}

impl ConnectionBezier {
    pub const DEFAULT_POINTS: usize = 25;

    pub fn new(feather: f32) -> Self {
        Self {
            feather,
            ..Default::default()
        }
    }

    pub fn mesh(&self) -> &Mesh {
        self.polyline.mesh()
    }

    pub fn points(&self) -> &[Pos2] {
        self.polyline.points()
    }

    pub fn update_points(&mut self, start: Pos2, end: Pos2, scale: f32) {
        let needs_rebuild = !self.inited
            || pos_changed_p2(self.start, start)
            || pos_changed_p2(self.end, end)
            || scale_changed(self.scale, scale);
        if !needs_rebuild {
            return;
        }

        self.inited = true;
        self.start = start;
        self.end = end;
        self.scale = scale;
        self.points_dirty = true;

        let points = self.polyline.points_mut();
        if points.len() != Self::DEFAULT_POINTS {
            points.resize(Self::DEFAULT_POINTS, Pos2::ZERO);
        }
        bezier_helper::sample(points.as_mut_slice(), start, end, scale);
    }

    pub fn show(
        &mut self,
        gui: &mut Gui<'_>,
        sense: Sense,
        id_salt: impl std::hash::Hash,
        hovered: bool,
        broke: bool,
    ) -> Response {
        let style = if broke {
            ConnectionBezierStyle {
                start_color: gui.style.connections.broke_clr,
                end_color: gui.style.connections.broke_clr,
                stroke_width: gui.style.connections.stroke_width,
            }
        } else if hovered {
            ConnectionBezierStyle {
                start_color: gui.style.node.output_hover_color,
                end_color: gui.style.node.input_hover_color,
                stroke_width: gui.style.connections.stroke_width,
            }
        } else {
            ConnectionBezierStyle {
                start_color: gui.style.node.output_port_color,
                end_color: gui.style.node.input_port_color,
                stroke_width: gui.style.connections.stroke_width,
            }
        };
        self.rebuild_mesh_if_needed(style, hovered, broke);
        let hover_scale = gui.style.connections.hover_distance_scale;
        let pointer_pos = gui.ui().input(|input| input.pointer.hover_pos());
        let hit = pointer_pos.is_some_and(|pos| self.hit_test(pos, hover_scale));

        let id = gui.ui().make_persistent_id(id_salt);
        let response = if hit {
            let rect = points_bounds(self.polyline.points())
                .map(|rect| rect.expand(self.stroke_width * 0.5))
                .unwrap_or(Rect::NOTHING);
            gui.ui().interact(rect, id, sense)
        } else {
            gui.ui().interact(Rect::NOTHING, id, Sense::hover())
        };
        self.polyline.render(&gui.painter());
        response
    }

    pub fn intersects_breaker(&self, breaker: Option<&ConnectionBreaker>) -> bool {
        let Some(breaker) = breaker else {
            return false;
        };
        let points = self.polyline.points();
        for (b1, b2) in breaker.segments() {
            let curve_segments = points.windows(2).map(|pair| (pair[0], pair[1]));
            for (a1, a2) in curve_segments {
                if bezier_helper::segments_intersect(a1, a2, b1, b2) {
                    return true;
                }
            }
        }
        false
    }

    fn hit_test(&self, pos: Pos2, hover_scale: f32) -> bool {
        let width = self.stroke_width;
        if width <= 0.0 {
            return false;
        }
        assert!(hover_scale.is_finite() && hover_scale >= 0.0);
        let points = self.polyline.points();
        if points.len() < 2 {
            return false;
        }

        let threshold_sq = width * width * hover_scale;
        points
            .windows(2)
            .any(|segment| distance_sq_point_segment(pos, segment[0], segment[1]) <= threshold_sq)
    }

    fn rebuild_mesh_if_needed(&mut self, style: ConnectionBezierStyle, hovered: bool, broke: bool) {
        let style_changed = self.hovered != hovered || self.broke != broke;
        if !self.points_dirty && !style_changed {
            return;
        }

        assert!(style.stroke_width.is_finite() && style.stroke_width >= 0.0);

        self.hovered = hovered;
        self.broke = broke;
        self.stroke_width = style.stroke_width;
        self.polyline.rebuild(
            style.start_color,
            style.end_color,
            style.stroke_width,
            self.feather,
        );
        self.points_dirty = false;
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

impl Default for ConnectionBezier {
    fn default() -> Self {
        Self {
            polyline: PolylineMesh::with_point_capacity(ConnectionBezier::DEFAULT_POINTS),
            stroke_width: 0.0,
            start: Pos2::ZERO,
            end: Pos2::ZERO,
            scale: 1.0,
            inited: false,

            hovered: false,
            broke: false,
            points_dirty: false,
            feather: 0.0,
        }
    }
}

impl PartialEq for ConnectionBezierStyle {
    fn eq(&self, other: &Self) -> bool {
        self.start_color == other.start_color
            && self.end_color == other.end_color
            && !scale_changed(self.stroke_width, other.stroke_width)
    }
}
