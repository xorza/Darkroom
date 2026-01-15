use egui::epaint::Mesh;
use egui::{Color32, Pos2, Rect, Response, Sense};

use crate::common::{UiEquals, bezier_helper};
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::PortKind;
use crate::gui::polyline_mesh::PolylineMesh;
use crate::gui::style::Style;
use crate::gui::{Gui, style};

const DEFAULT_FEATHER: f32 = 0.8;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConnectionBezierStyle {
    pub(crate) start_color: Color32,
    pub(crate) end_color: Color32,
    pub(crate) stroke_width: f32,
    pub(crate) feather: f32,
}

#[derive(Debug, Clone)]
pub struct ConnectionBezier {
    polyline: PolylineMesh,

    inited: bool,
    points_dirty: bool,

    start: Pos2,
    end: Pos2,
    scale: f32,
    built_style: Option<ConnectionBezierStyle>,
    bounding_rect: Rect,
}

impl ConnectionBezier {
    pub const DEFAULT_POINTS: usize = 35;

    pub fn mesh(&self) -> &Mesh {
        self.polyline.mesh()
    }

    pub fn points(&self) -> &[Pos2] {
        self.polyline.points()
    }

    pub fn update_points(&mut self, start: Pos2, end: Pos2, scale: f32) {
        let needs_rebuild = !self.inited
            || !self.start.ui_equals(start)
            || !self.end.ui_equals(end)
            || !self.scale.ui_equals(scale);
        if !needs_rebuild {
            return;
        }

        self.inited = true;
        self.points_dirty = true;

        self.start = start;
        self.end = end;
        self.scale = scale;

        let points = self.polyline.points_mut();
        if points.len() != Self::DEFAULT_POINTS {
            points.resize(Self::DEFAULT_POINTS, Pos2::ZERO);
        }
        bezier_helper::sample(points.as_mut_slice(), start, end, scale);

        self.bounding_rect = points_bounds(points).unwrap_or(Rect::NOTHING);
    }

    pub fn show(
        &mut self,
        gui: &mut Gui<'_>,
        sense: Sense,
        id_salt: impl std::hash::Hash,
        style: ConnectionBezierStyle,
    ) -> Response {
        let id = gui.ui().make_persistent_id(id_salt);

        let expanded_rect = self.bounding_rect.expand(style.stroke_width * 0.5);
        if !gui.ui().is_rect_visible(expanded_rect) {
            return gui.ui().interact(Rect::NOTHING, id, Sense::hover());
        }

        self.rebuild_mesh_if_needed(style);

        let pointer_pos = gui.ui().input(|input| input.pointer.hover_pos());
        let hit = pointer_pos.is_some_and(|pos| {
            self.hit_test(
                pos,
                gui.style.connections.hover_detection_width * gui.scale(),
            )
        });

        let response = if hit {
            gui.ui().interact(expanded_rect, id, sense)
        } else {
            gui.ui().interact(Rect::NOTHING, id, Sense::hover())
        };
        self.polyline.render(gui.painter());
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

    fn hit_test(&self, pos: Pos2, hover_detection_width: f32) -> bool {
        assert!(hover_detection_width.is_finite() && hover_detection_width >= 0.0);
        if hover_detection_width <= 0.0 {
            return false;
        }
        let points = self.polyline.points();
        if points.len() < 2 {
            return false;
        }

        let threshold_sq = hover_detection_width * hover_detection_width;
        points
            .windows(2)
            .any(|segment| distance_sq_point_segment(pos, segment[0], segment[1]) <= threshold_sq)
    }

    fn rebuild_mesh_if_needed(&mut self, style: ConnectionBezierStyle) {
        assert!(style.stroke_width.is_finite() && style.stroke_width >= 0.0);
        assert!(style.feather.is_finite() && style.feather >= 0.0);

        if !self.points_dirty && self.built_style == Some(style) {
            return;
        }

        self.built_style = Some(style);
        self.points_dirty = false;

        self.polyline.rebuild(
            style.start_color,
            style.end_color,
            style.stroke_width,
            style.feather,
        );
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

            built_style: None,

            start: Pos2::ZERO,
            end: Pos2::ZERO,
            scale: 1.0,

            inited: false,
            points_dirty: false,
            bounding_rect: Rect::NOTHING,
        }
    }
}

impl PartialEq for ConnectionBezierStyle {
    fn eq(&self, other: &Self) -> bool {
        self.start_color == other.start_color
            && self.end_color == other.end_color
            && self.stroke_width.ui_equals(other.stroke_width)
            && self.feather.ui_equals(other.feather)
    }
}

impl Eq for ConnectionBezierStyle {}

impl ConnectionBezierStyle {
    pub fn build(
        style: &Style,
        port_kind: PortKind,
        broke: bool,
        hovered: bool,
    ) -> ConnectionBezierStyle {
        assert!(
            style.connections.stroke_width.is_finite() && style.connections.stroke_width >= 0.0,
            "connection stroke width must be finite and non-negative"
        );
        assert!(
            style.connections.feather.is_finite() && style.connections.feather >= 0.0,
            "connection feather must be finite and non-negative"
        );

        if broke {
            ConnectionBezierStyle {
                start_color: style.connections.broke_clr,
                end_color: style.connections.broke_clr,
                stroke_width: style.connections.stroke_width,
                feather: style.connections.feather,
            }
        } else if hovered {
            match port_kind {
                PortKind::Input | PortKind::Output => ConnectionBezierStyle {
                    start_color: style.node.output_hover_color,
                    end_color: style.node.input_hover_color,
                    stroke_width: style.connections.stroke_width,
                    feather: style.connections.feather,
                },
                PortKind::Trigger | PortKind::Event => ConnectionBezierStyle {
                    start_color: style.node.event_hover_color,
                    end_color: style.node.trigger_hover_color,
                    stroke_width: style.connections.stroke_width,
                    feather: style.connections.feather,
                },
            }
        } else {
            match port_kind {
                PortKind::Input | PortKind::Output => ConnectionBezierStyle {
                    start_color: style.node.output_port_color,
                    end_color: style.node.input_port_color,
                    stroke_width: style.connections.stroke_width,
                    feather: style.connections.feather,
                },
                PortKind::Trigger | PortKind::Event => ConnectionBezierStyle {
                    start_color: style.node.event_port_color,
                    end_color: style.node.trigger_port_color,
                    stroke_width: style.connections.stroke_width,
                    feather: style.connections.feather,
                },
            }
        }
    }
}
