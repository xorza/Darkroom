use egui::{Color32, Pos2, Rect, Response, Sense, pos2};

use crate::common::polyline_mesh::PolylineMesh;
use crate::common::{StableId, UiEquals, bezier_helper};
use crate::gui::Gui;
use crate::gui::graph_ui::connections::breaker::ConnectionBreaker;
use crate::gui::graph_ui::port::PortKind;
use crate::gui::style::Style;
use crate::gui::widgets::HitRegion;

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
    /// Endpoints (or scale, or target point count) changed since the
    /// last `ensure_sampled` — the polyline needs re-sampling.
    needs_sample: bool,
    /// Polyline points or built style changed since the last
    /// `rebuild_mesh_if_needed` — the mesh needs rebuilding.
    mesh_dirty: bool,
    target_point_count: usize,

    start: Pos2,
    end: Pos2,
    scale: f32,
    built_style: Option<ConnectionBezierStyle>,
}

impl ConnectionBezier {
    pub const DEFAULT_POINTS: usize = 35;

    pub fn update_points(&mut self, start: Pos2, end: Pos2, scale: f32) {
        self.update_points_with_count(start, end, scale, Self::DEFAULT_POINTS);
    }

    /// Record new endpoints + scale + target point count. Sampling is
    /// deferred to [`Self::ensure_sampled`] — typically the curve is
    /// off-screen and `show` early-returns via the cheap-bbox
    /// visibility check before sampling ever happens.
    pub fn update_points_with_count(
        &mut self,
        start: Pos2,
        end: Pos2,
        scale: f32,
        point_count: usize,
    ) {
        assert!(point_count >= 2, "point count must be at least 2");

        let changed = !self.inited
            || !self.start.ui_equals(start)
            || !self.end.ui_equals(end)
            || !self.scale.ui_equals(scale)
            || self.target_point_count != point_count;
        if !changed {
            return;
        }

        self.inited = true;
        self.start = start;
        self.end = end;
        self.scale = scale;
        self.target_point_count = point_count;
        self.needs_sample = true;
        // After re-sample, the mesh will need rebuilding too.
        self.mesh_dirty = true;
    }

    pub fn show(
        &mut self,
        gui: &mut Gui<'_>,
        sense: Sense,
        id_salt: impl std::hash::Hash,
        style: ConnectionBezierStyle,
    ) -> Response {
        let id = StableId::new(("connection_bezier", id_salt));

        // Cheap bbox from endpoints + control-point offsets — the
        // convex hull of the four cubic-Bezier control points is a
        // conservative superset of the curve. Lets us cull off-screen
        // curves without sampling.
        let expanded_rect = self.cheap_bounds(style.stroke_width);
        if !gui.is_rect_visible(expanded_rect) {
            return HitRegion::new(id).interact(gui);
        }

        self.ensure_sampled();
        self.rebuild_mesh_if_needed(style);

        let pointer_pos = gui.pointer_hover_pos();
        let hit = pointer_pos.is_some_and(|pos| {
            self.hit_test(
                pos,
                gui.style.connections.hover_detection_width * gui.scale(),
            )
        });

        let response = if hit {
            HitRegion::new(id)
                .rect(expanded_rect)
                .sense(sense)
                .interact(gui)
        } else {
            HitRegion::new(id).interact(gui)
        };
        self.polyline.render(gui.painter());
        response
    }

    pub fn intersects_breaker(&mut self, breaker: Option<&ConnectionBreaker>) -> bool {
        let Some(breaker) = breaker else {
            return false;
        };
        self.ensure_sampled();
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

    /// Convex-hull bbox of the cubic-Bezier control points, expanded
    /// by half stroke width. Conservative: the curve never extends
    /// outside this box. Cheap: no sampling.
    fn cheap_bounds(&self, stroke_width: f32) -> Rect {
        let offset = bezier_helper::control_offset(self.start, self.end, self.scale);
        let p1x = self.start.x + offset;
        let p2x = self.end.x - offset;
        let min_x = self.start.x.min(self.end.x).min(p1x).min(p2x);
        let max_x = self.start.x.max(self.end.x).max(p1x).max(p2x);
        let min_y = self.start.y.min(self.end.y);
        let max_y = self.start.y.max(self.end.y);
        Rect::from_min_max(pos2(min_x, min_y), pos2(max_x, max_y)).expand(stroke_width * 0.5)
    }

    fn ensure_sampled(&mut self) {
        if !self.needs_sample {
            return;
        }
        self.needs_sample = false;

        let count = self.target_point_count;
        let points = self.polyline.points_mut();
        if points.len() != count {
            points.resize(count, Pos2::ZERO);
        }
        bezier_helper::sample(points.as_mut_slice(), self.start, self.end, self.scale);
    }

    fn rebuild_mesh_if_needed(&mut self, style: ConnectionBezierStyle) {
        assert!(style.stroke_width.is_finite() && style.stroke_width >= 0.0);
        assert!(style.feather.is_finite() && style.feather >= 0.0);

        if !self.mesh_dirty && self.built_style == Some(style) {
            return;
        }

        self.built_style = Some(style);
        self.mesh_dirty = false;

        self.polyline.rebuild(
            style.start_color,
            style.end_color,
            style.stroke_width,
            style.feather,
        );
    }
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
            target_point_count: ConnectionBezier::DEFAULT_POINTS,

            inited: false,
            needs_sample: false,
            mesh_dirty: false,
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
        } else {
            // Connections go from source (Output/Event) to target (Input/Trigger)
            let (source_kind, target_kind) = match port_kind {
                PortKind::Input | PortKind::Output => (PortKind::Output, PortKind::Input),
                PortKind::Trigger | PortKind::Event => (PortKind::Event, PortKind::Trigger),
            };

            ConnectionBezierStyle {
                start_color: style.node.port_colors(source_kind).select(hovered),
                end_color: style.node.port_colors(target_kind).select(hovered),
                stroke_width: style.connections.stroke_width,
                feather: style.connections.feather,
            }
        }
    }
}
