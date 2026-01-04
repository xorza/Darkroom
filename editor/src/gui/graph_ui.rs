use eframe::egui;
use egui::{Key, Pos2, Rect, Ui, Vec2};
use graph::graph::NodeId;
use graph::prelude::{Binding, FuncLib, PortAddress};
use hashbrown::HashMap;

use crate::gui::connection_ui::{
    ConnectionBreaker, ConnectionKey, ConnectionRenderer, PortKind, draw_temporary_connection,
};
use crate::gui::node_ui::NodeLayout;
use crate::model::graph_view;
use crate::{
    gui::{node_ui, render::RenderContext},
    model,
};
use std::collections::HashSet;

const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;
const MAX_BREAKER_LENGTH: f32 = 900.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PortRef {
    node_id: NodeId,
    port_idx: usize,
    kind: PortKind,
}

#[derive(Debug, Clone)]
pub struct PortInfo {
    port: PortRef,
    center: Pos2,
}

#[derive(Debug)]
struct ConnectionDrag {
    pub active: bool,
    start_port: PortRef,
    start_pos: Pos2,
    current_pos: Pos2,
}

impl Default for ConnectionDrag {
    fn default() -> Self {
        let placeholder = PortRef {
            node_id: NodeId::nil(),
            port_idx: 0,
            kind: PortKind::Output,
        };
        Self {
            active: false,
            start_port: placeholder,
            start_pos: Pos2::ZERO,
            current_pos: Pos2::ZERO,
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

#[derive(Debug, Default)]
pub struct GraphUiInteraction {
    pub actions: Vec<(NodeId, GraphUiAction)>,
}

impl GraphUiInteraction {
    pub fn clear(&mut self) {
        self.actions.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphUiAction {
    CacheToggled,
    InputChanged,
    NodeRemoved,
}

#[derive(Debug)]
pub struct GraphLayout {
    pub origin: Pos2,
    pub scale: f32,
    pub node_layout: node_ui::NodeLayout,
    pub node_widths: HashMap<NodeId, f32>,
    pub ports: Vec<PortInfo>,
}

impl GraphUi {
    pub fn reset(&mut self) {
        self.connection_breaker.reset();
        self.connection_drag.reset();
    }

    pub fn render(
        &mut self,
        ui: &mut Ui,
        view_graph: &mut model::ViewGraph,
        func_lib: &FuncLib,
        ui_interaction: &mut GraphUiInteraction,
    ) {
        let ctx = RenderContext::new(ui);

        background(&ctx, view_graph.zoom, view_graph.pan);

        let pointer_pos = ctx
            .ui
            .input(|input| input.pointer.hover_pos())
            .and_then(|pos| {
                if ctx.rect.contains(pos) {
                    Some(pos)
                } else {
                    None
                }
            });
        let primary_pressed =
            pointer_pos.is_some() && ctx.ui.input(|input| input.pointer.primary_pressed());
        let primary_down =
            pointer_pos.is_some() && ctx.ui.input(|input| input.pointer.primary_down());
        let primary_released =
            pointer_pos.is_some() && ctx.ui.input(|input| input.pointer.primary_released());

        if let Some(pointer_pos) = pointer_pos {
            update_zoom_and_pan(&ctx, pointer_pos, view_graph);
        }

        let graph_layout = GraphLayout::build(&ctx, view_graph, func_lib);

        let port_activation = (ctx.style.port_radius * 1.6).max(10.0);
        let hovered_port = graph_layout.hovered_port(pointer_pos, ctx.rect, port_activation);
        let pointer_over_node =
            graph_layout.pointer_over_node(pointer_pos, ctx.rect, view_graph, func_lib);

        let connection_breaker = &mut self.connection_breaker;
        let connection_drag = &mut self.connection_drag;

        if !connection_breaker.active
            && !connection_drag.active
            && primary_pressed
            && pointer_pos.is_some()
            && !pointer_over_node
            && hovered_port.is_none()
        {
            view_graph.selected_node_id = None;
            connection_breaker.active = true;
            connection_breaker.points.clear();
            if let Some(pos) = pointer_pos {
                connection_breaker.points.push(pos);
            }
        }

        if !connection_breaker.active
            && !connection_drag.active
            && primary_pressed
            && pointer_pos.is_some()
            && let Some(port) = hovered_port.as_ref()
        {
            connection_drag.start(port.clone());
        }

        if connection_breaker.active
            && primary_down
            && let Some(pos) = pointer_pos
        {
            let should_add = connection_breaker
                .points
                .last()
                .map(|last| last.distance(pos) > 2.0)
                .unwrap_or(true);
            if should_add {
                let remaining =
                    MAX_BREAKER_LENGTH - breaker_path_length(&connection_breaker.points);
                let last_pos = connection_breaker.points.last().copied().unwrap_or(pos);
                let segment_len = last_pos.distance(pos);
                if remaining > 0.0 && segment_len > 0.0 {
                    if segment_len <= remaining {
                        connection_breaker.points.push(pos);
                    } else {
                        let t = remaining / segment_len;
                        let clamped = Pos2::new(
                            last_pos.x + (pos.x - last_pos.x) * t,
                            last_pos.y + (pos.y - last_pos.y) * t,
                        );
                        connection_breaker.points.push(clamped);
                    }
                }
            }
        }

        let mut connections = ConnectionRenderer::default();

        connections.rebuild(
            view_graph,
            func_lib,
            graph_layout.origin,
            &graph_layout.node_layout,
            &graph_layout.node_widths,
            connection_breaker,
        );
        connections.render(&ctx);

        if connection_breaker.active && connection_breaker.points.len() > 1 {
            ctx.painter.add(egui::Shape::line(
                connection_breaker.points.clone(),
                ctx.style.breaker_stroke,
            ));
        }

        let node_interaction =
            node_ui::render_node_bodies(&ctx, &graph_layout, view_graph, func_lib);

        ui_interaction.actions.extend(node_interaction.actions);
        if let Some(node_id) = node_interaction.remove_request {
            view_graph.remove_node(node_id);
        }

        if connection_drag.active {
            if let Some(pos) = pointer_pos {
                connection_drag.current_pos = pos;
            }
            let end_pos = hovered_port
                .as_ref()
                .filter(|&port| port.port.kind != connection_drag.start_port.kind)
                .map(|port| port.center)
                .unwrap_or(connection_drag.current_pos);
            draw_temporary_connection(
                &ctx.painter,
                view_graph.zoom,
                connection_drag.start_pos,
                end_pos,
                connection_drag.start_port.kind,
                &ctx.style,
            );
        }

        if connection_breaker.active && primary_released {
            let removed = remove_connections(view_graph, connections.highlighted());
            for node_id in removed {
                ui_interaction
                    .actions
                    .push((node_id, GraphUiAction::InputChanged));
            }
            connection_breaker.reset();
        }

        if connection_drag.active && primary_released {
            if let Some(target) = hovered_port.as_ref()
                && target.port.kind != connection_drag.start_port.kind
                && port_in_activation_range(
                    &connection_drag.current_pos,
                    target.center,
                    port_activation,
                )
                && let Some(node_id) = apply_connection(
                    view_graph,
                    func_lib,
                    connection_drag.start_port,
                    target.port,
                )
            {
                ui_interaction
                    .actions
                    .push((node_id, GraphUiAction::InputChanged));
            }
            connection_drag.reset();
        }

        if let Some(selected_id) = node_interaction.selection_request {
            view_graph.select_node(selected_id);
        }

        let mut fit_all = false;
        let mut view_selected = false;

        ctx.ui.horizontal(|ui| {
            fit_all = ui.button("Fit all").clicked();
            view_selected = ui.button("View selected").clicked();
            let reset_view = ui.button("Reset view").clicked();
            if reset_view {
                view_graph.zoom = 1.0;
                view_graph.pan = egui::Vec2::ZERO;
            }
        });
        if view_selected {
            view_selected_node(&ctx, &graph_layout, view_graph, func_lib);
        }
        if fit_all {
            fit_all_nodes(&ctx, &graph_layout, view_graph, func_lib);
        }
    }
}

fn update_zoom_and_pan(ctx: &RenderContext, cursor_pos: Pos2, view_graph: &mut model::ViewGraph) {
    let scroll_delta = ctx.ui.input(|input| input.smooth_scroll_delta);
    let pinch_delta = ctx.ui.input(|input| {
        if input.modifiers.command {
            1.0
        } else {
            input.zoom_delta()
        }
    });

    let (zoom_delta, pan) = if scroll_delta.x.abs() > f32::EPSILON {
        (1.0, scroll_delta)
    } else {
        (
            (scroll_delta.length_sq() * 0.006).exp() * pinch_delta,
            Vec2::ZERO,
        )
    };

    if (zoom_delta - 1.0).abs() > f32::EPSILON {
        let clamped_zoom = (view_graph.zoom * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);

        if (clamped_zoom - view_graph.zoom).abs() > f32::EPSILON {
            let origin = ctx.rect.min;
            let graph_pos = (cursor_pos - origin - view_graph.pan) / view_graph.zoom;

            view_graph.zoom = clamped_zoom;
            view_graph.pan = cursor_pos - origin - graph_pos * view_graph.zoom;
        }
    }

    view_graph.pan += pan;

    let pan_id = ctx.ui.make_persistent_id("graph_pan");
    let pan_response = ctx.ui.interact(ctx.rect, pan_id, egui::Sense::drag());
    if pan_response.dragged_by(egui::PointerButton::Middle) {
        view_graph.pan += pan_response.drag_delta();
    }
}

impl GraphLayout {
    fn build(
        ctx: &RenderContext,
        view_graph: &model::ViewGraph,
        func_lib: &FuncLib,
    ) -> GraphLayout {
        let origin = ctx.rect.min + view_graph.pan;
        let node_layout = node_ui::NodeLayout::default().scaled(view_graph.zoom);
        let width_ctx = node_ui::NodeWidthContext {
            node_layout: &node_layout,
            style: &ctx.style,
            scale: view_graph.zoom,
        };
        let node_widths =
            node_ui::compute_node_widths(&ctx.painter, view_graph, func_lib, &width_ctx);

        let ports = collect_ports(view_graph, func_lib, &node_widths, origin, &node_layout);
        GraphLayout {
            origin,
            scale: view_graph.zoom,
            node_layout,
            node_widths,
            ports,
        }
    }

    pub fn node_width(&self, node_id: NodeId) -> f32 {
        self.node_widths
            .get(&node_id)
            .copied()
            .expect("node width must be precomputed")
    }

    pub fn node_rect(
        &self,
        view_node: &model::ViewNode,
        input_count: usize,
        output_count: usize,
    ) -> Rect {
        node_ui::node_rect_for_graph(
            self.origin,
            view_node,
            input_count,
            output_count,
            self.scale,
            &self.node_layout,
            self.node_width(view_node.id),
        )
    }

    pub fn hovered_port(
        &self,
        pointer_pos: Option<Pos2>,
        rect: Rect,
        activation_radius: f32,
    ) -> Option<PortInfo> {
        assert!(rect.is_finite(), "graph rect must be finite");
        pointer_pos
            .filter(|pos| rect.contains(*pos))
            .and_then(|pos| find_port_near(&self.ports, pos, activation_radius))
    }

    pub fn pointer_over_node(
        &self,
        pointer_pos: Option<Pos2>,
        rect: Rect,
        view_graph: &model::ViewGraph,
        func_lib: &FuncLib,
    ) -> bool {
        assert!(rect.is_finite(), "graph rect must be finite");
        pointer_pos
            .filter(|pos| rect.contains(*pos))
            .is_some_and(|pos| {
                view_graph.view_nodes.iter().any(|view_node| {
                    let node = view_graph.graph.by_id(&view_node.id).unwrap();
                    let func = func_lib.by_id(&node.func_id).unwrap();
                    let node_rect =
                        self.node_rect(view_node, func.inputs.len(), func.outputs.len());
                    node_rect.contains(pos)
                })
            })
    }
}

fn background(ctx: &RenderContext, zoom: f32, pan: Vec2) {
    let spacing = ctx.style.dotted_base_spacing * zoom;
    let radius = (ctx.style.dotted_radius_base * zoom)
        .clamp(ctx.style.dotted_radius_min, ctx.style.dotted_radius_max);
    let color = ctx.style.dotted_color;
    let origin = ctx.rect.min + pan;
    let offset_x = (ctx.rect.left() - origin.x).rem_euclid(spacing);
    let offset_y = (ctx.rect.top() - origin.y).rem_euclid(spacing);
    let start_x = ctx.rect.left() - offset_x - spacing;
    let start_y = ctx.rect.top() - offset_y - spacing;

    let mut y = start_y;
    while y <= ctx.rect.bottom() + spacing {
        let mut x = start_x;
        while x <= ctx.rect.right() + spacing {
            ctx.painter.circle_filled(Pos2::new(x, y), radius, color);
            x += spacing;
        }
        y += spacing;
    }
}

fn collect_ports(
    view_graph: &model::ViewGraph,
    func_lib: &FuncLib,
    node_widths: &HashMap<NodeId, f32>,
    origin: Pos2,
    node_layout: &NodeLayout,
) -> Vec<PortInfo> {
    let mut ports = Vec::new();

    for node_view in view_graph.view_nodes.iter().rev() {
        let node = view_graph.graph.by_id(&node_view.id).unwrap();
        let func = func_lib.by_id(&node.func_id).unwrap();
        let node_width = node_widths.get(&node.id).copied().unwrap();

        for index in 0..func.inputs.len() {
            let center = node_ui::node_input_pos(
                origin,
                node_view,
                index,
                func.inputs.len(),
                node_layout,
                view_graph.zoom,
            );

            ports.push(PortInfo {
                port: PortRef {
                    node_id: node.id,
                    port_idx: index,
                    kind: PortKind::Input,
                },
                center,
            });
        }
        for index in 0..func.outputs.len() {
            let center = node_ui::node_output_pos(
                origin,
                node_view,
                index,
                node_layout,
                view_graph.zoom,
                node_width,
            );

            ports.push(PortInfo {
                port: PortRef {
                    node_id: node.id,
                    port_idx: index,
                    kind: PortKind::Output,
                },
                center,
            });
        }
    }

    ports
}

fn find_port_near(ports: &[PortInfo], pos: Pos2, radius: f32) -> Option<PortInfo> {
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

fn port_in_activation_range(cursor: &Pos2, port_center: Pos2, radius: f32) -> bool {
    assert!(radius.is_finite(), "port activation radius must be finite");
    assert!(radius > 0.0, "port activation radius must be positive");
    cursor.distance(port_center) <= radius
}

fn apply_connection(
    view_graph: &mut model::ViewGraph,
    func_lib: &FuncLib,
    start: PortRef,
    end: PortRef,
) -> Option<NodeId> {
    assert!(start.kind != end.kind, "ports must be of opposite types");
    let (output_port, input_port) = match (start.kind, end.kind) {
        (PortKind::Output, PortKind::Input) => (start, end),
        (PortKind::Input, PortKind::Output) => (end, start),
        _ => {
            return None;
        }
    };

    let output_node = view_graph
        .graph
        .by_id(&output_port.node_id)
        .expect("output node must exist");
    let output_func = func_lib.by_id(&output_node.func_id).unwrap_or_else(|| {
        panic!(
            "Missing func for node {} ({})",
            output_node.name, output_node.func_id
        )
    });
    assert!(
        output_port.port_idx < output_func.outputs.len(),
        "output index must be valid for output node"
    );

    let input_node = view_graph
        .graph
        .by_id_mut(&input_port.node_id)
        .expect("input node must exist");
    let input_func = func_lib.by_id(&input_node.func_id).unwrap_or_else(|| {
        panic!(
            "Missing func for node {} ({})",
            input_node.name, input_node.func_id
        )
    });
    assert!(
        input_port.port_idx < input_func.inputs.len(),
        "input index must be valid for input node"
    );
    input_node.inputs[input_port.port_idx].binding = Binding::Bind(PortAddress {
        target_id: output_port.node_id,
        port_idx: output_port.port_idx,
    });
    Some(input_node.id)
}

fn view_selected_node(
    ctx: &RenderContext,
    graph_layout: &GraphLayout,
    view_graph: &mut model::ViewGraph,
    func_lib: &FuncLib,
) {
    let Some(selected_id) = view_graph.selected_node_id else {
        return;
    };
    let Some(node_view) = view_graph
        .view_nodes
        .iter()
        .find(|node| node.id == selected_id)
    else {
        return;
    };

    let node = view_graph
        .graph
        .by_id(&node_view.id)
        .expect("node view id must exist in graph data");
    let func = func_lib
        .by_id(&node.func_id)
        .unwrap_or_else(|| panic!("Missing func for node {} ({})", node.name, node.func_id));

    let node_width = graph_layout
        .node_widths
        .get(&node.id)
        .copied()
        .expect("node width must be precomputed");
    let size = node_ui::node_rect_for_graph(
        Pos2::ZERO,
        node_view,
        func.inputs.len(),
        func.outputs.len(),
        1.0,
        &graph_layout.node_layout,
        node_width,
    )
    .size();
    let center = node_view.pos.to_vec2() + size * 0.5;
    view_graph.zoom = 1.0;
    view_graph.pan = ctx.rect.center() - ctx.rect.min - center;
}

fn fit_all_nodes(
    ctx: &RenderContext,
    graph_layout: &GraphLayout,
    view_graph: &mut model::ViewGraph,
    func_lib: &FuncLib,
) {
    if view_graph.view_nodes.is_empty() {
        view_graph.zoom = 1.0;
        view_graph.pan = egui::Vec2::ZERO;
        return;
    }

    let mut min = Pos2::new(f32::INFINITY, f32::INFINITY);
    let mut max = Pos2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);

    for node_view in &view_graph.view_nodes {
        let node = view_graph
            .graph
            .by_id(&node_view.id)
            .expect("node view id must exist in graph data");
        let func = func_lib
            .by_id(&node.func_id)
            .unwrap_or_else(|| panic!("Missing func for node {} ({})", node.name, node.func_id));
        let node_width = graph_layout
            .node_widths
            .get(&node.id)
            .copied()
            .expect("node width must be precomputed");
        let rect = node_ui::node_rect_for_graph(
            Pos2::ZERO,
            node_view,
            func.inputs.len(),
            func.outputs.len(),
            1.0,
            &graph_layout.node_layout,
            node_width,
        );
        min.x = min.x.min(rect.min.x);
        min.y = min.y.min(rect.min.y);
        max.x = max.x.max(rect.max.x);
        max.y = max.y.max(rect.max.y);
    }

    let bounds_size = max - min;
    assert!(bounds_size.x.is_finite(), "bounds width must be finite");
    assert!(bounds_size.y.is_finite(), "bounds height must be finite");

    let padding = 24.0;
    let available = ctx.rect.size() - egui::vec2(padding * 2.0, padding * 2.0);
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
    view_graph.zoom = target_zoom;

    let bounds_center = (min.to_vec2() + max.to_vec2()) * 0.5;
    view_graph.pan = ctx.rect.center() - ctx.rect.min - bounds_center * view_graph.zoom;
}

fn remove_connections(
    view_graph: &mut model::ViewGraph,
    highlighted: &HashSet<ConnectionKey>,
) -> HashSet<NodeId> {
    let mut affected = HashSet::new();
    if highlighted.is_empty() {
        return affected;
    }
    for node in view_graph.graph.nodes.iter_mut() {
        for (input_index, input) in node.inputs.iter_mut().enumerate() {
            let key = ConnectionKey {
                target_node_id: node.id,
                input_index,
            };
            if highlighted.contains(&key) {
                input.binding = Binding::None;
                affected.insert(node.id);
            }
        }
    }
    affected
}

fn breaker_path_length(points: &[Pos2]) -> f32 {
    points
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum()
}
