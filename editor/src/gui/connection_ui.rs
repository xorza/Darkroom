use eframe::egui;
use egui::Pos2;
use egui::epaint::CubicBezierShape;
use graph::graph::NodeId;
use graph::prelude::Binding;
use std::collections::HashSet;

use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::graph_layout::GraphLayout;
use crate::gui::{graph_ctx::GraphContext, node_ui};
use crate::model;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ConnectionKey {
    pub(crate) input_node_id: NodeId,
    pub(crate) input_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PortKind {
    Input,
    Output,
}

#[derive(Debug, Clone)]
struct ConnectionCurve {
    key: ConnectionKey,
    start: Pos2,
    end: Pos2,
    control_offset: f32,
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionUi {
    curves: Vec<ConnectionCurve>,
    pub highlighted: HashSet<ConnectionKey>,
}

impl ConnectionUi {
    pub(crate) fn rebuild(
        &mut self,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
        breaker: Option<&ConnectionBreaker>,
    ) {
        self.collect_curves(ctx, graph_layout, view_graph);

        self.highlighted.clear();
        if let Some(breaker) = breaker {
            self.collect_highlighted(breaker);
        }
    }

    pub(crate) fn render(
        &mut self,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
        breaker: Option<&ConnectionBreaker>,
    ) {
        self.rebuild(ctx, graph_layout, view_graph, breaker);

        for curve in &self.curves {
            let stroke = if self.highlighted.contains(&curve.key) {
                ctx.style.connection_highlight_stroke
            } else {
                ctx.style.connection_stroke
            };
            let control_offset = curve.control_offset;
            let shape = CubicBezierShape::from_points_stroke(
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
            ctx.painter.add(shape);
        }
    }

    fn collect_curves(
        &mut self,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
    ) {
        self.curves.clear();

        for node_view in &view_graph.view_nodes {
            let node = view_graph.graph.by_id(&node_view.id).unwrap();

            for (input_index, input) in node.inputs.iter().enumerate() {
                let Binding::Bind(binding) = &input.binding else {
                    continue;
                };
                let start_layout = node_ui::compute_node_layout(
                    ctx,
                    &binding.target_id,
                    &graph_layout.node_layout,
                    graph_layout.origin,
                );
                let end_layout = node_ui::compute_node_layout(
                    ctx,
                    &node.id,
                    &graph_layout.node_layout,
                    graph_layout.origin,
                );
                let start = start_layout.output_center(binding.port_idx);
                let end = end_layout.input_center(input_index);
                let control_offset = node_ui::bezier_control_offset(start, end, view_graph.scale);
                self.curves.push(ConnectionCurve {
                    key: ConnectionKey {
                        input_node_id: node.id,
                        input_idx: input_index,
                    },
                    start,
                    end,
                    control_offset,
                });
            }
        }
    }

    fn collect_highlighted(&mut self, breaker: &ConnectionBreaker) {
        if breaker.points.len() < 2 {
            return;
        }

        let breaker_segments = breaker.points.windows(2).map(|pair| (pair[0], pair[1]));

        for curve in self.curves.iter() {
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
                self.highlighted.insert(curve.key);
            }
        }
    }
}

fn sample_cubic_bezier(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, steps: usize) -> Vec<Pos2> {
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
        points.push(Pos2::new(x, y));
    }
    points
}

fn segments_intersect(a1: Pos2, a2: Pos2, b1: Pos2, b2: Pos2) -> bool {
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

fn orient(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn on_segment(a: Pos2, b: Pos2, p: Pos2) -> bool {
    let min_x = a.x.min(b.x);
    let max_x = a.x.max(b.x);
    let min_y = a.y.min(b.y);
    let max_y = a.y.max(b.y);
    p.x >= min_x - 1e-6 && p.x <= max_x + 1e-6 && p.y >= min_y - 1e-6 && p.y <= max_y + 1e-6
}
