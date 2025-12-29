use eframe::egui;

use crate::{
    gui::{
        node,
        render::{RenderContext, WidgetRenderer},
    },
    model,
};
use std::collections::HashSet;
use uuid::Uuid;

const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;
const MAX_BREAKER_LENGTH: f32 = 900.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ConnectionKey {
    target_node_id: Uuid,
    input_index: usize,
}

#[derive(Debug, Default)]
struct ConnectionBreaker {
    pub active: bool,
    pub points: Vec<egui::Pos2>,
}

impl ConnectionBreaker {
    pub fn reset(&mut self) {
        self.active = false;
        self.points.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortKind {
    Input,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PortRef {
    node_id: Uuid,
    index: usize,
    kind: PortKind,
}

#[derive(Debug, Clone)]
struct PortInfo {
    port: PortRef,
    center: egui::Pos2,
}

#[derive(Debug)]
struct ConnectionDrag {
    pub active: bool,
    start_port: PortRef,
    start_pos: egui::Pos2,
    current_pos: egui::Pos2,
}

impl Default for ConnectionDrag {
    fn default() -> Self {
        let placeholder = PortRef {
            node_id: Uuid::nil(),
            index: 0,
            kind: PortKind::Output,
        };
        Self {
            active: false,
            start_port: placeholder,
            start_pos: egui::Pos2::ZERO,
            current_pos: egui::Pos2::ZERO,
        }
    }
}

impl ConnectionDrag {
    fn start(&mut self, port: PortInfo) {
        self.active = true;
        self.start_port = port.port;
        self.start_pos = port.center;
        self.current_pos = port.center;
    }

    pub fn reset(&mut self) {
        self.active = false;
    }
}

#[derive(Debug, Default)]
pub struct GraphUi {
    connection_breaker: ConnectionBreaker,
    connection_drag: ConnectionDrag,
}

impl GraphUi {
    pub fn reset(&mut self) {
        self.connection_breaker.reset();
        self.connection_drag.reset();
    }

    pub fn render(&mut self, ui: &mut egui::Ui, graph: &mut model::GraphView) {
        let breaker = &mut self.connection_breaker;
        let connection_drag = &mut self.connection_drag;

        let mut fit_all = false;
        let mut view_selected = false;
        let mut reset_view = false;
        ui.horizontal(|ui| {
            fit_all = ui.button("Fit all").clicked();
            view_selected = ui.button("View selected").clicked();
            reset_view = ui.button("Reset view").clicked();
        });

        let rect = ui.available_rect_before_wrap();
        let painter = ui.painter_at(rect);
        let input_ctx = RenderContext::new(ui, &painter, rect, graph);

        if reset_view {
            graph.zoom = 1.0;
            graph.pan = egui::Vec2::ZERO;
        }

        if view_selected {
            view_selected_node(ui, &painter, rect, graph);
        }

        if fit_all {
            fit_all_nodes(ui, &painter, rect, graph);
        }

        let pointer_pos = ui.input(|input| input.pointer.hover_pos());
        let cursor_pos = ui.ctx().pointer_latest_pos().or(pointer_pos);
        let pointer_in_rect = pointer_pos
            .map(|pos| input_ctx.rect.contains(pos))
            .unwrap_or(false);
        let middle_down = ui.input(|input| input.pointer.middle_down());
        let pointer_delta = ui.input(|input| input.pointer.delta());
        let port_activation = (input_ctx.port_radius * 1.6).max(10.0);
        let ports = collect_ports(
            graph,
            input_ctx.origin,
            &input_ctx.layout,
            &input_ctx.node_widths,
        );
        let hovered_port = pointer_pos
            .filter(|pos| input_ctx.rect.contains(*pos))
            .and_then(|pos| find_port_near(&ports, pos, port_activation));
        let hovered_port_ref = hovered_port.as_ref();
        let pointer_over_node = pointer_pos
            .filter(|pos| input_ctx.rect.contains(*pos))
            .is_some_and(|pos| {
                graph.nodes.iter().any(|node| {
                    let node_rect = input_ctx.node_rect(node);
                    node_rect.contains(pos)
                })
            });
        let pan_id = ui.make_persistent_id("graph_pan");
        let pan_response = ui.interact(
            input_ctx.rect,
            pan_id,
            if breaker.active
                || connection_drag.active
                || pointer_over_node
                || hovered_port.is_some()
            {
                egui::Sense::hover()
            } else {
                egui::Sense::drag()
            },
        );

        if pan_response.dragged_by(egui::PointerButton::Primary)
            && !pointer_over_node
            && !breaker.active
            && !connection_drag.active
        {
            graph.pan += pan_response.drag_delta();
        }
        if middle_down && pointer_in_rect && !breaker.active && !connection_drag.active {
            assert!(
                pointer_delta.x.is_finite(),
                "pointer delta x must be finite"
            );
            assert!(
                pointer_delta.y.is_finite(),
                "pointer delta y must be finite"
            );
            graph.pan += pointer_delta;
        }

        let primary_pressed = ui.input(|input| input.pointer.primary_pressed());
        let primary_down = ui.input(|input| input.pointer.primary_down());
        let primary_released = ui.input(|input| input.pointer.primary_released());

        if !breaker.active
            && !connection_drag.active
            && primary_pressed
            && pointer_in_rect
            && !pointer_over_node
            && hovered_port.is_none()
        {
            graph.selected_node_id = None;
            breaker.active = true;
            breaker.points.clear();
            if let Some(pos) = pointer_pos {
                breaker.points.push(pos);
            }
        }

        if !breaker.active
            && !connection_drag.active
            && primary_pressed
            && pointer_in_rect
            && let Some(port) = hovered_port_ref
        {
            connection_drag.start(port.clone());
        }

        if breaker.active
            && primary_down
            && let Some(pos) = pointer_pos
        {
            let should_add = breaker
                .points
                .last()
                .map(|last| last.distance(pos) > 2.0)
                .unwrap_or(true);
            if should_add {
                let remaining = MAX_BREAKER_LENGTH - breaker_path_length(&breaker.points);
                let last_pos = breaker.points.last().copied().unwrap_or(pos);
                let segment_len = last_pos.distance(pos);
                if remaining > 0.0 && segment_len > 0.0 {
                    if segment_len <= remaining {
                        breaker.points.push(pos);
                    } else {
                        let t = remaining / segment_len;
                        let clamped = egui::pos2(
                            last_pos.x + (pos.x - last_pos.x) * t,
                            last_pos.y + (pos.y - last_pos.y) * t,
                        );
                        breaker.points.push(clamped);
                    }
                }
            }
        }

        let zoom_active = cursor_pos.is_some_and(|pos| input_ctx.rect.contains(pos));

        if zoom_active {
            let modifiers = ui.input(|input| input.modifiers);
            let scroll_delta = ui.input(|input| input.raw_scroll_delta);
            let mut zoom_delta = ui.input(|input| input.zoom_delta());
            let wheel_delta = ui.input(|input| {
                input
                    .events
                    .iter()
                    .fold(egui::Vec2::ZERO, |acc, event| match event {
                        egui::Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page,
                            delta,
                            ..
                        } => acc + *delta,
                        _ => acc,
                    })
            });
            let wheel_scroll = wheel_delta.length_sq() > f32::EPSILON;
            assert!(scroll_delta.x.is_finite(), "scroll delta x must be finite");
            assert!(scroll_delta.y.is_finite(), "scroll delta y must be finite");
            assert!(wheel_delta.x.is_finite(), "wheel delta x must be finite");
            assert!(wheel_delta.y.is_finite(), "wheel delta y must be finite");

            if wheel_scroll && wheel_delta.y.abs() > f32::EPSILON {
                let wheel_zoom = (wheel_delta.y * 0.06).exp();
                assert!(wheel_zoom.is_finite(), "wheel zoom factor must be finite");
                zoom_delta *= wheel_zoom;
            } else if (modifiers.command || modifiers.ctrl) && scroll_delta.y.abs() > f32::EPSILON {
                let scroll_zoom = (scroll_delta.y * 0.003).exp();
                assert!(scroll_zoom.is_finite(), "scroll zoom factor must be finite");
                zoom_delta *= scroll_zoom;
            }

            if (zoom_delta - 1.0).abs() > f32::EPSILON {
                let clamped_zoom = (graph.zoom * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
                assert!(clamped_zoom.is_finite(), "clamped zoom must be finite");

                if (clamped_zoom - graph.zoom).abs() > f32::EPSILON {
                    let cursor = cursor_pos.expect("cursor position must exist while zooming");
                    assert!(
                        input_ctx.rect.contains(cursor),
                        "cursor must be inside graph rect while zooming"
                    );
                    let origin = input_ctx.rect.min;
                    let graph_pos = (cursor - origin - graph.pan) / graph.zoom;

                    graph.zoom = clamped_zoom;
                    graph.pan = cursor - origin - graph_pos * graph.zoom;
                }
            } else if !wheel_scroll && scroll_delta.length_sq() > f32::EPSILON {
                graph.pan += scroll_delta;
            }
        }

        let ctx = RenderContext::new(ui, &painter, rect, graph);
        let render_origin = ctx.rect.min + graph.pan;
        let mut background = BackgroundRenderer;
        let mut connections = ConnectionRenderer::default();
        let mut node_bodies = NodeBodyRenderer;

        background.render(&ctx, graph);
        connections.rebuild(graph, render_origin, &ctx.layout, &ctx.node_widths, breaker);
        connections.render(&ctx, graph);

        if breaker.active && breaker.points.len() > 1 {
            ctx.painter().add(egui::Shape::line(
                breaker.points.clone(),
                ctx.style.breaker_stroke,
            ));
        }

        let interaction = node_bodies.render(&ctx, graph);
        if let Some(node_id) = interaction.remove_request {
            graph.remove_node(node_id);
        }

        if connection_drag.active {
            if let Some(pos) = pointer_pos {
                connection_drag.current_pos = pos;
            }
            let end_pos = hovered_port_ref
                .filter(|port| port.port.kind != connection_drag.start_port.kind)
                .map(|port| port.center)
                .unwrap_or(connection_drag.current_pos);
            draw_temporary_connection(
                ctx.painter(),
                graph.zoom,
                connection_drag.start_pos,
                end_pos,
                connection_drag.start_port.kind,
                &ctx.style,
            );
        }

        if breaker.active && primary_released {
            remove_connections(graph, connections.highlighted());
            breaker.reset();
        }

        if connection_drag.active && primary_released {
            if let Some(target) = hovered_port_ref
                && target.port.kind != connection_drag.start_port.kind
                && port_in_activation_range(
                    &connection_drag.current_pos,
                    target.center,
                    port_activation,
                )
            {
                apply_connection(graph, connection_drag.start_port, target.port);
            }
            connection_drag.reset();
        }

        if let Some(selected_id) = interaction.selection_request {
            graph.select_node(selected_id);
        }
    }
}

#[derive(Debug)]
struct BackgroundRenderer;

impl WidgetRenderer for BackgroundRenderer {
    type Output = ();

    fn render(&mut self, ctx: &RenderContext, graph: &mut model::GraphView) -> Self::Output {
        draw_dotted_background(ctx.painter(), ctx.rect, graph, &ctx.style);
    }
}

#[derive(Debug, Default)]
struct ConnectionRenderer {
    curves: Vec<ConnectionCurve>,
    highlighted: HashSet<ConnectionKey>,
}

impl ConnectionRenderer {
    fn rebuild(
        &mut self,
        graph: &model::GraphView,
        origin: egui::Pos2,
        layout: &node::NodeLayout,
        node_widths: &std::collections::HashMap<Uuid, f32>,
        breaker: &ConnectionBreaker,
    ) {
        self.curves = collect_connection_curves(graph, origin, layout, node_widths);
        self.highlighted = if breaker.active && breaker.points.len() > 1 {
            connection_hits(&self.curves, &breaker.points)
        } else {
            HashSet::new()
        };
    }

    fn highlighted(&self) -> &HashSet<ConnectionKey> {
        &self.highlighted
    }
}

impl WidgetRenderer for ConnectionRenderer {
    type Output = ();

    fn render(&mut self, ctx: &RenderContext, _graph: &mut model::GraphView) -> Self::Output {
        draw_connections(ctx.painter(), &self.curves, &self.highlighted, &ctx.style);
    }
}

#[derive(Debug)]
struct NodeBodyRenderer;

impl WidgetRenderer for NodeBodyRenderer {
    type Output = node::NodeInteraction;

    fn render(&mut self, ctx: &RenderContext, graph: &mut model::GraphView) -> Self::Output {
        node::render_node_bodies(ctx, graph)
    }
}

fn draw_dotted_background(
    painter: &egui::Painter,
    rect: egui::Rect,
    graph: &model::GraphView,
    style: &crate::gui::style::GraphStyle,
) {
    let spacing = style.dotted_base_spacing * graph.zoom;
    let radius = (style.dotted_radius_base * graph.zoom)
        .clamp(style.dotted_radius_min, style.dotted_radius_max);
    let color = style.dotted_color;

    assert!(spacing.is_finite(), "dot spacing must be finite");
    assert!(spacing > 0.0, "dot spacing must be positive");
    assert!(radius.is_finite(), "dot radius must be finite");
    assert!(radius > 0.0, "dot radius must be positive");

    let origin = rect.min + graph.pan;
    let offset_x = (rect.left() - origin.x).rem_euclid(spacing);
    let offset_y = (rect.top() - origin.y).rem_euclid(spacing);
    let start_x = rect.left() - offset_x - spacing;
    let start_y = rect.top() - offset_y - spacing;

    let mut y = start_y;
    while y <= rect.bottom() + spacing {
        let mut x = start_x;
        while x <= rect.right() + spacing {
            painter.circle_filled(egui::pos2(x, y), radius, color);
            x += spacing;
        }
        y += spacing;
    }
}

#[derive(Debug, Clone)]
struct ConnectionCurve {
    key: ConnectionKey,
    start: egui::Pos2,
    end: egui::Pos2,
    control_offset: f32,
}

fn collect_connection_curves(
    graph: &model::GraphView,
    origin: egui::Pos2,
    layout: &node::NodeLayout,
    node_widths: &std::collections::HashMap<Uuid, f32>,
) -> Vec<ConnectionCurve> {
    let node_lookup: std::collections::HashMap<_, _> =
        graph.nodes.iter().map(|node| (node.id, node)).collect();
    let mut curves = Vec::new();

    for node in &graph.nodes {
        for (input_index, input) in node.inputs.iter().enumerate() {
            let Some(connection) = &input.connection else {
                continue;
            };
            let source_node = node_lookup
                .get(&connection.node_id)
                .expect("graph validation must guarantee source nodes exist");
            let source_width = node_widths
                .get(&connection.node_id)
                .copied()
                .expect("node width must be precomputed");
            let start = node::node_output_pos(
                origin,
                source_node,
                connection.output_index,
                layout,
                graph.zoom,
                source_width,
            );
            let end = node::node_input_pos(origin, node, input_index, layout, graph.zoom);
            let control_offset = node::bezier_control_offset(start, end, graph.zoom);
            curves.push(ConnectionCurve {
                key: ConnectionKey {
                    target_node_id: node.id,
                    input_index,
                },
                start,
                end,
                control_offset,
            });
        }
    }

    curves
}

fn collect_ports(
    graph: &model::GraphView,
    origin: egui::Pos2,
    layout: &node::NodeLayout,
    node_widths: &std::collections::HashMap<Uuid, f32>,
) -> Vec<PortInfo> {
    let mut ports = Vec::new();

    for node in graph.nodes.iter().rev() {
        let node_width = node_widths
            .get(&node.id)
            .copied()
            .expect("node width must be precomputed");
        for (index, _input) in node.inputs.iter().enumerate() {
            let center = node::node_input_pos(origin, node, index, layout, graph.zoom);

            ports.push(PortInfo {
                port: PortRef {
                    node_id: node.id,
                    index,
                    kind: PortKind::Input,
                },
                center,
            });
        }
        for (index, _output) in node.outputs.iter().enumerate() {
            let center = node::node_output_pos(origin, node, index, layout, graph.zoom, node_width);

            ports.push(PortInfo {
                port: PortRef {
                    node_id: node.id,
                    index,
                    kind: PortKind::Output,
                },
                center,
            });
        }
    }

    ports
}

fn find_port_near(ports: &[PortInfo], pos: egui::Pos2, radius: f32) -> Option<PortInfo> {
    assert!(radius.is_finite(), "port activation radius must be finite");
    assert!(radius > 0.0, "port activation radius must be positive");
    let mut best = None;
    let mut best_dist = radius;

    for port in ports {
        let dist = port.center.distance(pos);
        if dist < best_dist {
            best_dist = dist;
            best = Some(port.clone());
        }
    }

    best
}

fn draw_temporary_connection(
    painter: &egui::Painter,
    scale: f32,
    start: egui::Pos2,
    end: egui::Pos2,
    start_kind: PortKind,
    style: &crate::gui::style::GraphStyle,
) {
    assert!(scale.is_finite(), "connection scale must be finite");
    assert!(scale > 0.0, "connection scale must be positive");
    let control_offset = node::bezier_control_offset(start, end, scale);
    let (start_sign, end_sign) = match start_kind {
        PortKind::Output => (1.0, -1.0),
        PortKind::Input => (-1.0, 1.0),
    };
    let stroke = style.temp_connection_stroke;
    let shape = egui::epaint::CubicBezierShape::from_points_stroke(
        [
            start,
            start + egui::vec2(control_offset * start_sign, 0.0),
            end + egui::vec2(control_offset * end_sign, 0.0),
            end,
        ],
        false,
        egui::Color32::TRANSPARENT,
        stroke,
    );
    painter.add(shape);
}

fn port_in_activation_range(cursor: &egui::Pos2, port_center: egui::Pos2, radius: f32) -> bool {
    assert!(radius.is_finite(), "port activation radius must be finite");
    assert!(radius > 0.0, "port activation radius must be positive");
    cursor.distance(port_center) <= radius
}

fn apply_connection(graph: &mut model::GraphView, start: PortRef, end: PortRef) {
    assert!(start.kind != end.kind, "ports must be of opposite types");
    let (output_port, input_port) = match (start.kind, end.kind) {
        (PortKind::Output, PortKind::Input) => (start, end),
        (PortKind::Input, PortKind::Output) => (end, start),
        _ => {
            return;
        }
    };

    let output_node = graph
        .nodes
        .iter()
        .find(|node| node.id == output_port.node_id)
        .expect("output node must exist");
    assert!(
        output_port.index < output_node.outputs.len(),
        "output index must be valid for output node"
    );

    let input_node = graph
        .nodes
        .iter_mut()
        .find(|node| node.id == input_port.node_id)
        .expect("input node must exist");
    assert!(
        input_port.index < input_node.inputs.len(),
        "input index must be valid for input node"
    );
    input_node.inputs[input_port.index].connection = Some(model::Connection {
        node_id: output_port.node_id,
        output_index: output_port.index,
    });
}

fn view_selected_node(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    graph: &mut model::GraphView,
) {
    let Some(selected_id) = graph.selected_node_id else {
        return;
    };
    let Some(node) = graph.nodes.iter().find(|node| node.id == selected_id) else {
        return;
    };

    let (layout, node_widths) = compute_layout_and_widths(ui, painter, graph, 1.0);
    let node_width = node_widths
        .get(&node.id)
        .copied()
        .expect("node width must be precomputed");
    let size = node::node_rect_for_graph(egui::Pos2::ZERO, node, 1.0, &layout, node_width).size();
    let center = node.pos.to_vec2() + size * 0.5;
    graph.zoom = 1.0;
    graph.pan = rect.center() - rect.min - center;
}

fn fit_all_nodes(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    graph: &mut model::GraphView,
) {
    if graph.nodes.is_empty() {
        graph.zoom = 1.0;
        graph.pan = egui::Vec2::ZERO;
        return;
    }

    let (layout, node_widths) = compute_layout_and_widths(ui, painter, graph, 1.0);
    let mut min = egui::pos2(f32::INFINITY, f32::INFINITY);
    let mut max = egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);

    for node in &graph.nodes {
        let node_width = node_widths
            .get(&node.id)
            .copied()
            .expect("node width must be precomputed");
        let rect = node::node_rect_for_graph(egui::Pos2::ZERO, node, 1.0, &layout, node_width);
        min.x = min.x.min(rect.min.x);
        min.y = min.y.min(rect.min.y);
        max.x = max.x.max(rect.max.x);
        max.y = max.y.max(rect.max.y);
    }

    let bounds_size = max - min;
    assert!(bounds_size.x.is_finite(), "bounds width must be finite");
    assert!(bounds_size.y.is_finite(), "bounds height must be finite");

    let padding = 24.0;
    let available = rect.size() - egui::vec2(padding * 2.0, padding * 2.0);
    let zoom_x = if bounds_size.x > 0.0 {
        available.x / bounds_size.x
    } else {
        1.0
    };
    let zoom_y = if bounds_size.y > 0.0 {
        available.y / bounds_size.y
    } else {
        1.0
    };
    let target_zoom = zoom_x.min(zoom_y).clamp(MIN_ZOOM, MAX_ZOOM);
    graph.zoom = target_zoom;

    let bounds_center = (min.to_vec2() + max.to_vec2()) * 0.5;
    graph.pan = rect.center() - rect.min - bounds_center * graph.zoom;
}

fn compute_layout_and_widths(
    ui: &egui::Ui,
    painter: &egui::Painter,
    graph: &model::GraphView,
    scale: f32,
) -> (node::NodeLayout, std::collections::HashMap<Uuid, f32>) {
    let layout = node::NodeLayout::default().scaled(scale);
    layout.assert_valid();
    let heading_font = node::scaled_font(ui, egui::TextStyle::Heading, scale);
    let body_font = node::scaled_font(ui, egui::TextStyle::Body, scale);
    let text_color = ui.visuals().text_color();
    let style = crate::gui::style::GraphStyle::new(ui, scale);
    style.validate();
    let widths = node::compute_node_widths(
        painter,
        graph,
        &layout,
        &heading_font,
        &body_font,
        text_color,
        &style,
    );
    (layout, widths)
}
fn draw_connections(
    painter: &egui::Painter,
    curves: &[ConnectionCurve],
    highlighted: &HashSet<ConnectionKey>,
    style: &crate::gui::style::GraphStyle,
) {
    for curve in curves {
        let stroke = if highlighted.contains(&curve.key) {
            style.connection_highlight_stroke
        } else {
            style.connection_stroke
        };
        let control_offset = curve.control_offset;
        let shape = egui::epaint::CubicBezierShape::from_points_stroke(
            [
                curve.start,
                curve.start + egui::vec2(control_offset, 0.0),
                curve.end + egui::vec2(-control_offset, 0.0),
                curve.end,
            ],
            false,
            egui::Color32::TRANSPARENT,
            stroke,
        );
        painter.add(shape);
    }
}

fn connection_hits(curves: &[ConnectionCurve], breaker: &[egui::Pos2]) -> HashSet<ConnectionKey> {
    let mut hits = HashSet::new();
    let breaker_segments = breaker.windows(2).map(|pair| (pair[0], pair[1]));

    for curve in curves {
        let samples = sample_cubic_bezier(
            curve.start,
            curve.start + egui::vec2(curve.control_offset, 0.0),
            curve.end + egui::vec2(-curve.control_offset, 0.0),
            curve.end,
            24,
        );
        let curve_segments = samples.windows(2).map(|pair| (pair[0], pair[1]));
        let mut hit = false;
        for (a1, a2) in breaker_segments.clone() {
            for (b1, b2) in curve_segments.clone() {
                if segments_intersect(a1, a2, b1, b2) {
                    hit = true;
                    break;
                }
            }
            if hit {
                break;
            }
        }
        if hit {
            hits.insert(curve.key);
        }
    }

    hits
}

fn sample_cubic_bezier(
    p0: egui::Pos2,
    p1: egui::Pos2,
    p2: egui::Pos2,
    p3: egui::Pos2,
    steps: usize,
) -> Vec<egui::Pos2> {
    assert!(steps >= 2, "bezier sampling steps must be at least 2");
    let mut points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let one_minus = 1.0 - t;
        let a = one_minus * one_minus * one_minus;
        let b = 3.0 * one_minus * one_minus * t;
        let c = 3.0 * one_minus * t * t;
        let d = t * t * t;
        let x = a * p0.x + b * p1.x + c * p2.x + d * p3.x;
        let y = a * p0.y + b * p1.y + c * p2.y + d * p3.y;
        points.push(egui::pos2(x, y));
    }
    points
}

fn segments_intersect(a1: egui::Pos2, a2: egui::Pos2, b1: egui::Pos2, b2: egui::Pos2) -> bool {
    let o1 = orient(a1, a2, b1);
    let o2 = orient(a1, a2, b2);
    let o3 = orient(b1, b2, a1);
    let o4 = orient(b1, b2, a2);
    let eps = 1e-6;

    if o1.abs() < eps && on_segment(a1, a2, b1) {
        return true;
    }
    if o2.abs() < eps && on_segment(a1, a2, b2) {
        return true;
    }
    if o3.abs() < eps && on_segment(b1, b2, a1) {
        return true;
    }
    if o4.abs() < eps && on_segment(b1, b2, a2) {
        return true;
    }

    (o1 > 0.0) != (o2 > 0.0) && (o3 > 0.0) != (o4 > 0.0)
}

fn orient(a: egui::Pos2, b: egui::Pos2, c: egui::Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn on_segment(a: egui::Pos2, b: egui::Pos2, p: egui::Pos2) -> bool {
    let min_x = a.x.min(b.x);
    let max_x = a.x.max(b.x);
    let min_y = a.y.min(b.y);
    let max_y = a.y.max(b.y);
    p.x >= min_x - 1e-6 && p.x <= max_x + 1e-6 && p.y >= min_y - 1e-6 && p.y <= max_y + 1e-6
}

fn remove_connections(graph: &mut model::GraphView, highlighted: &HashSet<ConnectionKey>) {
    if highlighted.is_empty() {
        return;
    }
    for node in &mut graph.nodes {
        for (input_index, input) in node.inputs.iter_mut().enumerate() {
            let key = ConnectionKey {
                target_node_id: node.id,
                input_index,
            };
            if highlighted.contains(&key) {
                input.connection = None;
            }
        }
    }
}

fn breaker_path_length(points: &[egui::Pos2]) -> f32 {
    points
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum()
}
