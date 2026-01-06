use eframe::egui;
use egui::Pos2;
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
    start: Pos2,
    end: Pos2,
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionUi {
    curves: Vec<ConnectionCurve>,
    pub(crate) highlighted: HashSet<ConnectionKey>,
    pub(crate) drag: Option<ConnectionDrag>,

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
        let row_height = ctx.style.node_row_height * view_graph.scale;
        self.collect_curves(graph_layout, view_graph, row_height);

        self.highlighted.clear();
        if let Some(breaker) = breaker {
            self.collect_highlighted(breaker, view_graph.scale);
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
            ConnectionBezier::sample(
                &mut self.point_cache,
                curve.start,
                curve.end,
                view_graph.scale,
            );
            ctx.painter.line(self.point_cache.clone(), stroke);
        }

        if let Some(drag) = &self.drag {
            let (start, end) = match drag.start_port.port.kind {
                PortKind::Output => (drag.current_pos, drag.start_port.center),
                PortKind::Input => (drag.start_port.center, drag.current_pos),
            };
            ConnectionBezier::sample(&mut self.point_cache, start, end, view_graph.scale);
            ctx.painter
                .line(self.point_cache.clone(), ctx.style.temp_connection_stroke);
        }
    }

    pub(crate) fn start_drag(&mut self, port: PortInfo) {
        self.drag = Some(ConnectionDrag::new(port));
    }

    pub(crate) fn update_drag(
        &mut self,
        ctx: &GraphContext,
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
                    .filter(|end_port| {
                        end_port.center.distance(drag.current_pos)
                            < ctx.style.port_activation_radius
                    })
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

    fn collect_curves(
        &mut self,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
        row_height: f32,
    ) {
        self.curves.clear();

        for node_view in &view_graph.view_nodes {
            let node = view_graph.graph.by_id(&node_view.id).unwrap();

            for (input_index, input) in node.inputs.iter().enumerate() {
                let Binding::Bind(binding) = &input.binding else {
                    continue;
                };
                let start_layout = graph_layout.node_layout(&binding.target_id);
                let end_layout = graph_layout.node_layout(&node.id);

                let start = end_layout.input_center(input_index, row_height);
                let end = start_layout.output_center(binding.port_idx, row_height);

                self.curves.push(ConnectionCurve {
                    key: ConnectionKey {
                        input_node_id: node.id,
                        input_idx: input_index,
                    },
                    start,
                    end,
                });
            }
        }
    }

    fn collect_highlighted(&mut self, breaker: &ConnectionBreaker, scale: f32) {
        if breaker.points.len() < 2 {
            return;
        }

        let breaker_segments = breaker.points.windows(2).map(|pair| (pair[0], pair[1]));

        for curve in self.curves.iter() {
            ConnectionBezier::sample(&mut self.point_cache, curve.start, curve.end, scale);

            let curve_segments = self.point_cache.windows(2).map(|pair| (pair[0], pair[1]));
            let mut hit = false;
            for (a1, a2) in breaker_segments.clone() {
                for (b1, b2) in curve_segments.clone() {
                    if ConnectionBezier::segments_intersect(a1, a2, b1, b2) {
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
