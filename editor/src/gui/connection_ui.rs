use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use eframe::egui;
use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{Color32, Pos2, Shape};
use graph::graph::NodeId;
use graph::prelude::Binding;
use std::collections::HashSet;
use std::sync::Arc;

use crate::common::connection_bezier::{self, ConnectionBezier};
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

    //cache
    points: Vec<Pos2>,
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
            points: Vec::with_capacity(connection_bezier::POINTS),
        }
    }
}

#[derive(Debug, Clone)]
struct ConnectionCurve {
    key: ConnectionKey,

    inited: bool,
    highlighted: bool,

    output_pos: Pos2,
    input_pos: Pos2,

    points: Vec<Pos2>,
    mesh: Mesh,
}

impl ConnectionCurve {
    fn new(key: ConnectionKey) -> Self {
        Self {
            key,
            inited: false,
            highlighted: false,
            output_pos: Pos2::ZERO,
            input_pos: Pos2::ZERO,
            points: Vec::with_capacity(connection_bezier::POINTS),
            mesh: mesh_with_capacity(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionUi {
    curves: KeyIndexVec<ConnectionKey, ConnectionCurve>,
    pub(crate) highlighted: HashSet<ConnectionKey>,
    pub(crate) drag: Option<ConnectionDrag>,

    //caches
    mesh: Arc<Mesh>,
}

impl ConnectionUi {
    pub(crate) fn render(
        &mut self,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
        breaker: Option<&ConnectionBreaker>,
    ) {
        self.rebuild(ctx, graph_layout, view_graph, breaker);

        ctx.painter.add(Shape::mesh(Arc::clone(&self.mesh)));
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

    fn rebuild(
        &mut self,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
        breaker: Option<&ConnectionBreaker>,
    ) {
        let pixels_per_point = ctx.ui.ctx().pixels_per_point();
        let feather = 1.0 / pixels_per_point;

        self.highlighted.clear();

        let mesh = Arc::get_mut(&mut self.mesh).unwrap();
        mesh.clear();

        let mut write_idx: usize = 0;

        for node_view in &view_graph.view_nodes {
            let node = view_graph.graph.by_id(&node_view.id).unwrap();

            for (input_idx, input) in node.inputs.iter().enumerate() {
                let Binding::Bind(binding) = &input.binding else {
                    continue;
                };
                let connection_key = ConnectionKey {
                    input_node_id: node.id,
                    input_idx,
                };
                let curve_idx = self
                    .curves
                    .compact_insert_with(&connection_key, &mut write_idx, || {
                        ConnectionCurve::new(connection_key)
                    });
                let curve = &mut self.curves[curve_idx];

                let output_layout = graph_layout.node_layout(&binding.target_id);
                let input_layout = graph_layout.node_layout(&node.id);

                let input_pos = input_layout.input_center(input_idx);
                let output_pos = output_layout.output_center(binding.port_idx);

                let needs_rebuild = !curve.inited
                    || curve.output_pos.distance_sq(output_pos) > 1.0
                    || curve.input_pos.distance_sq(input_pos) > 1.0;

                if needs_rebuild {
                    curve.output_pos = output_pos;
                    curve.input_pos = input_pos;
                    curve.inited = true;

                    curve.points.clear();
                    let _ = ConnectionBezier::sample(
                        &mut curve.points,
                        output_pos,
                        input_pos,
                        ctx.scale,
                    );
                }

                let highlighted = if let Some(segments) = breaker.and_then(|breaker| {
                    (!breaker.segments().is_empty()).then_some(breaker.segments())
                }) {
                    let curve_segments = curve.points.windows(2).map(|pair| (pair[0], pair[1]));
                    let mut hit = false;
                    'outer: for (b1, b2) in curve_segments {
                        for (a1, a2) in segments {
                            if ConnectionBezier::segments_intersect(*a1, *a2, b1, b2) {
                                self.highlighted.insert(connection_key);
                                hit = true;
                                break 'outer;
                            }
                        }
                    }

                    hit
                } else {
                    false
                };

                if curve.highlighted != highlighted || needs_rebuild {
                    curve.highlighted = highlighted;
                    curve.mesh.clear();

                    if curve.highlighted {
                        add_curve_to_mesh(
                            &mut curve.mesh,
                            &curve.points,
                            ctx.style.connections.highlight_stroke.color,
                            ctx.style.connections.highlight_stroke.color,
                            ctx.style.connections.highlight_stroke.width,
                            feather,
                        );
                    } else {
                        add_curve_to_mesh(
                            &mut curve.mesh,
                            &curve.points,
                            ctx.style.node.output_port_color,
                            ctx.style.node.input_port_color,
                            ctx.style.connections.stroke_width,
                            feather,
                        );
                    };
                }

                mesh.append_ref(&curve.mesh);
            }
        }

        self.curves.compact_finish(write_idx);

        if let Some(drag) = &mut self.drag {
            let (start, end) = match drag.start_port.port.kind {
                PortKind::Input => (drag.current_pos, drag.start_port.center),
                PortKind::Output => (drag.start_port.center, drag.current_pos),
            };
            drag.points.clear();
            let _ = ConnectionBezier::sample(&mut drag.points, start, end, ctx.scale);
            add_curve_to_mesh(
                mesh,
                &drag.points,
                ctx.style.node.output_port_color,
                ctx.style.node.input_port_color,
                ctx.style.connections.stroke_width,
                feather,
            );
        }
    }
}

fn add_curve_to_mesh(
    mesh: &mut Mesh,
    points: &[Pos2],
    start_color: Color32,
    end_color: Color32,
    width: f32,
    feather: f32,
) {
    assert!(points.len() >= 2);
    assert!(width > 0.0);
    assert!(feather >= 0.0);

    let segment_count = points.len() - 1;
    let half_width = width * 0.5;
    for (idx, segment) in points.windows(2).enumerate() {
        let a = segment[0];
        let b = segment[1];
        let dir = b - a;
        if dir.length_sq() <= f32::EPSILON {
            continue;
        }
        let normal = dir.normalized().rot90();
        let outer = half_width + feather;
        let t0 = idx as f32 / segment_count as f32;
        let t1 = (idx + 1) as f32 / segment_count as f32;
        let color0 = lerp_color(start_color, end_color, t0);
        let color1 = lerp_color(start_color, end_color, t1);
        let color0_outer = set_alpha(color0, 0);
        let color1_outer = set_alpha(color1, 0);

        let inner_plus0 = a + normal * half_width;
        let inner_minus0 = a - normal * half_width;
        let inner_plus1 = b + normal * half_width;
        let inner_minus1 = b - normal * half_width;
        let outer_plus0 = a + normal * outer;
        let outer_minus0 = a - normal * outer;
        let outer_plus1 = b + normal * outer;
        let outer_minus1 = b - normal * outer;

        add_quad(
            mesh,
            [inner_plus0, inner_minus0, inner_minus1, inner_plus1],
            [color0, color0, color1, color1],
        );
        add_quad(
            mesh,
            [outer_plus0, inner_plus0, inner_plus1, outer_plus1],
            [color0_outer, color0, color1, color1_outer],
        );
        add_quad(
            mesh,
            [inner_minus0, outer_minus0, outer_minus1, inner_minus1],
            [color0, color0_outer, color1_outer, color1],
        );
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

fn set_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn add_quad(mesh: &mut Mesh, positions: [Pos2; 4], colors: [Color32; 4]) {
    let base = mesh.vertices.len() as u32;
    mesh.vertices.push(Vertex {
        pos: positions[0],
        uv: WHITE_UV,
        color: colors[0],
    });
    mesh.vertices.push(Vertex {
        pos: positions[1],
        uv: WHITE_UV,
        color: colors[1],
    });
    mesh.vertices.push(Vertex {
        pos: positions[2],
        uv: WHITE_UV,
        color: colors[2],
    });
    mesh.vertices.push(Vertex {
        pos: positions[3],
        uv: WHITE_UV,
        color: colors[3],
    });
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn mesh_with_capacity() -> Mesh {
    let (vertex_capacity, index_capacity) = mesh_capacity(connection_bezier::POINTS);
    let mut mesh = Mesh::default();
    mesh.vertices.reserve(vertex_capacity);
    mesh.indices.reserve(index_capacity);
    mesh
}

fn mesh_capacity(points: usize) -> (usize, usize) {
    assert!(points >= 2, "bezier point count must be at least 2");
    let segments = points - 1;
    let quads_per_segment = 3;
    let vertices_per_quad = 4;
    let indices_per_quad = 6;
    (
        segments * quads_per_segment * vertices_per_quad,
        segments * quads_per_segment * indices_per_quad,
    )
}

impl KeyIndexKey<ConnectionKey> for ConnectionCurve {
    fn key(&self) -> &ConnectionKey {
        &self.key
    }
}
