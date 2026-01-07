use eframe::egui;
use egui::{Color32, Pos2, Stroke};
use graph::graph::NodeId;
use graph::prelude::Binding;
use std::collections::HashSet;

use crate::common::connection_bezier::ConnectionBezier;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::{GraphLayout, PortInfo};
use crate::gui::node_ui::PortDragInfo;
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

#[derive(Debug)]
pub(crate) struct ConnectionDrag {
    pub(crate) start_port: PortInfo,
    pub(crate) end_port: Option<PortInfo>,
    pub(crate) current_pos: Pos2,
}

#[derive(Debug, Clone)]
pub(crate) enum ConnectionDragUpdate {
    InProgress,
    Finished,
    FinishedWith {
        start_port: PortInfo,
        end_port: PortInfo,
    },
}

impl ConnectionDrag {
    pub(crate) fn new(port: PortInfo) -> Self {
        Self {
            current_pos: port.center,
            start_port: port,
            end_port: None,
        }
    }
}

#[derive(Debug, Clone)]
struct ConnectionCurve {
    key: ConnectionKey,
    start_idx: usize,
    end_idx: usize,
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionUi {
    curves: Vec<ConnectionCurve>,
    pub(crate) highlighted: HashSet<ConnectionKey>,
    pub(crate) drag: Option<ConnectionDrag>,

    //caches
    point_cache: Vec<Pos2>,
}

impl ConnectionUi {
    pub(crate) fn rebuild(
        &mut self,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
        breaker: Option<&ConnectionBreaker>,
    ) {
        self.curves.clear();

        self.collect_connection_curves(ctx, graph_layout, view_graph);

        self.highlighted.clear();
        if let Some(breaker) = breaker {
            self.collect_highlighted(breaker);
        }
    }

    fn collect_connection_curves(
        &mut self,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
    ) {
        for node_view in &view_graph.view_nodes {
            let node = view_graph.graph.by_id(&node_view.id).unwrap();

            for (input_index, input) in node.inputs.iter().enumerate() {
                let Binding::Bind(binding) = &input.binding else {
                    continue;
                };
                let start_layout = graph_layout.node_layout(&binding.target_id);
                let end_layout = graph_layout.node_layout(&node.id);

                let input_pos = end_layout.input_center(input_index);
                let output_pos = start_layout.output_center(binding.port_idx);

                let (start_idx, end_idx) = ConnectionBezier::sample(
                    &mut self.point_cache,
                    output_pos,
                    input_pos,
                    ctx.scale,
                );

                self.curves.push(ConnectionCurve {
                    key: ConnectionKey {
                        input_node_id: node.id,
                        input_idx: input_index,
                    },
                    start_idx,
                    end_idx,
                });
            }
        }
    }

    fn collect_highlighted(&mut self, breaker: &ConnectionBreaker) {
        let breaker_segments = breaker.segments();
        if breaker_segments.is_empty() {
            return;
        }

        for curve in self.curves.iter() {
            let curve_segments = self.point_cache[curve.start_idx..=curve.end_idx]
                .windows(2)
                .map(|pair| (pair[0], pair[1]));
            let mut hit = false;
            'outer: for (b1, b2) in curve_segments {
                for (a1, a2) in breaker_segments {
                    if ConnectionBezier::segments_intersect(*a1, *a2, b1, b2) {
                        hit = true;
                        break 'outer;
                    }
                }
            }
            if hit {
                self.highlighted.insert(curve.key);
            }
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
            let points = &self.point_cache[curve.start_idx..=curve.end_idx];
            if self.highlighted.contains(&curve.key) {
                ctx.painter
                    .line(points.to_vec(), ctx.style.connections.highlight_stroke);
            } else {
                self.draw_gradient_line(
                    ctx,
                    points,
                    ctx.style.node.output_port_color,
                    ctx.style.node.input_port_color,
                    ctx.style.connections.stroke_width,
                );
            };
        }

        if let Some(drag) = &self.drag {
            let (start, end) = match drag.start_port.port.kind {
                PortKind::Input => (drag.current_pos, drag.start_port.center),
                PortKind::Output => (drag.start_port.center, drag.current_pos),
            };

            let (start_idx, end_idx) =
                ConnectionBezier::sample(&mut self.point_cache, start, end, ctx.scale);
            self.draw_gradient_line(
                ctx,
                &self.point_cache[start_idx..=end_idx],
                ctx.style.node.output_port_color,
                ctx.style.node.input_port_color,
                ctx.style.connections.stroke_width,
            );
        }
    }

    pub(crate) fn start_drag(&mut self, port: PortInfo) {
        self.drag = Some(ConnectionDrag::new(port));
    }

    pub(crate) fn update_drag(
        &mut self,
        pointer_pos: Pos2,
        drag_port_info: PortDragInfo,
    ) -> ConnectionDragUpdate {
        let drag = self.drag.as_mut().unwrap();
        drag.current_pos = pointer_pos;

        match drag_port_info {
            PortDragInfo::None => ConnectionDragUpdate::InProgress,
            PortDragInfo::DragStart(_) => unreachable!(),
            PortDragInfo::Hover(port_info) => {
                if drag.start_port.port.kind != port_info.port.kind {
                    drag.end_port = Some(port_info);
                    drag.current_pos = port_info.center;
                }

                ConnectionDragUpdate::InProgress
            }
            PortDragInfo::DragStop => {
                let update = drag
                    .end_port
                    .map_or(ConnectionDragUpdate::Finished, |end_port| {
                        ConnectionDragUpdate::FinishedWith {
                            start_port: drag.start_port,
                            end_port,
                        }
                    });

                self.stop_drag();

                update
            }
        }
    }

    pub(crate) fn stop_drag(&mut self) {
        self.drag = None;
    }

    fn draw_gradient_line(
        &self,
        ctx: &GraphContext,
        points: &[Pos2],
        start_color: Color32,
        end_color: Color32,
        width: f32,
    ) {
        assert!(points.len() >= 2);

        let segment_count = points.len() - 1;
        for (idx, segment) in points.windows(2).enumerate() {
            let t = idx as f32 / segment_count as f32;
            let color = Self::lerp_color(start_color, end_color, t);
            ctx.painter
                .line_segment([segment[0], segment[1]], Stroke::new(width, color));
        }
    }

    fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
        let t = t.clamp(0.0, 1.0);
        let lerp = |start: u8, end: u8| -> u8 {
            (start as f32 + (end as f32 - start as f32) * t).round() as u8
        };
        Color32::from_rgba_unmultiplied(
            lerp(a.r(), b.r()),
            lerp(a.g(), b.g()),
            lerp(a.b(), b.b()),
            lerp(a.a(), b.a()),
        )
    }
}
