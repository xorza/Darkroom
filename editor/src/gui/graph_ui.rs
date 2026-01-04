use eframe::egui;
use egui::{Key, Pos2, Ui, Vec2};
use graph::graph::NodeId;
use graph::prelude::{Binding, FuncLib, PortAddress};

use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::{
    ConnectionKey, ConnectionRenderer, PortKind, draw_temporary_connection,
};
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};
use crate::model::graph_view;
use crate::{
    gui::{node_ui, render::RenderContext},
    model,
};
use std::collections::HashSet;

const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;
const MAX_BREAKER_LENGTH: f32 = 900.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum InteractionState {
    #[default]
    Idle,
    Breaking,
    DraggingNewConnection,
}

#[derive(Debug)]
struct ConnectionDrag {
    start_port: PortRef,
    start_pos: Pos2,
    current_pos: Pos2,
}

impl Default for ConnectionDrag {
    fn default() -> Self {
        let placeholder = PortRef {
            node_id: NodeId::nil(),
            idx: 0,
            kind: PortKind::Output,
        };
        Self {
            start_port: placeholder,
            start_pos: Pos2::ZERO,
            current_pos: Pos2::ZERO,
        }
    }
}

impl ConnectionDrag {
    fn start(&mut self, port: PortInfo) {
        self.start_port = port.port;
        self.start_pos = port.center;
        self.current_pos = port.center;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Default)]
pub struct GraphUi {
    state: InteractionState,
    connection_breaker: ConnectionBreaker,
    connection_drag: ConnectionDrag,
    connection_renderer: ConnectionRenderer,
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
    InputChanged { input_idx: usize },
    NodeRemoved,
    NodeSelected,
}

impl GraphUi {
    pub fn reset(&mut self) {
        self.state = InteractionState::Idle;
        self.connection_breaker.reset();
        self.connection_drag.reset();
        self.connection_renderer = ConnectionRenderer::default();
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

        let hovered_port = graph_layout.hovered_port(&ctx, pointer_pos, ctx.rect);
        let pointer_over_node = graph_layout.pointer_over_node(pointer_pos, view_graph, func_lib);

        if self.state == InteractionState::Idle
            && primary_pressed
            && !pointer_over_node
            && hovered_port.is_none()
        {
            view_graph.selected_node_id = None;
            self.state = InteractionState::Breaking;
            self.connection_breaker.reset();
            if let Some(pos) = pointer_pos {
                self.connection_breaker.points.push(pos);
            }
        }

        if self.state == InteractionState::Idle
            && primary_pressed
            && let Some(port) = hovered_port.as_ref()
        {
            self.connection_drag.start(port.clone());
            self.state = InteractionState::DraggingNewConnection;
        }

        if self.state == InteractionState::Breaking
            && primary_down
            && let Some(pos) = pointer_pos
        {
            let should_add = self
                .connection_breaker
                .points
                .last()
                .map(|last| last.distance(pos) > 2.0)
                .unwrap_or(true);
            if should_add {
                let remaining =
                    MAX_BREAKER_LENGTH - breaker_path_length(&self.connection_breaker.points);
                let last_pos = self
                    .connection_breaker
                    .points
                    .last()
                    .copied()
                    .unwrap_or(pos);
                let segment_len = last_pos.distance(pos);
                if remaining > 0.0 && segment_len > 0.0 {
                    if segment_len <= remaining {
                        self.connection_breaker.points.push(pos);
                    } else {
                        let t = remaining / segment_len;
                        let clamped = Pos2::new(
                            last_pos.x + (pos.x - last_pos.x) * t,
                            last_pos.y + (pos.y - last_pos.y) * t,
                        );
                        self.connection_breaker.points.push(clamped);
                    }
                }
            }
        }

        self.connection_renderer.render(
            &ctx,
            &graph_layout,
            view_graph,
            func_lib,
            if self.state == InteractionState::Breaking {
                Some(&self.connection_breaker)
            } else {
                None
            },
        );

        if self.state == InteractionState::Breaking {
            self.connection_breaker.render(&ctx);
        }

        if self.state == InteractionState::DraggingNewConnection {
            if let Some(pos) = pointer_pos {
                self.connection_drag.current_pos = pos;
            }
            let end_pos = hovered_port
                .as_ref()
                .filter(|&port| port.port.kind != self.connection_drag.start_port.kind)
                .map(|port| port.center)
                .unwrap_or(self.connection_drag.current_pos);
            draw_temporary_connection(
                &ctx.painter,
                view_graph.zoom,
                self.connection_drag.start_pos,
                end_pos,
                self.connection_drag.start_port.kind,
                &ctx.style,
            );
        }

        if self.state == InteractionState::Breaking && primary_released {
            for connection in self.connection_renderer.highlighted.iter() {
                if let Some(node) = view_graph.graph.nodes.by_key_mut(&connection.input_node_id) {
                    node.inputs[connection.input_idx].binding = Binding::None;
                    ui_interaction.actions.push((
                        connection.input_node_id,
                        GraphUiAction::InputChanged {
                            input_idx: connection.input_idx,
                        },
                    ));
                }
            }
            self.connection_breaker.reset();
            self.state = InteractionState::Idle;
        }

        if self.state == InteractionState::DraggingNewConnection && primary_released {
            if let Some(target) = hovered_port.as_ref()
                && target.port.kind != self.connection_drag.start_port.kind
                && port_in_activation_range(
                    &self.connection_drag.current_pos,
                    target.center,
                    ctx.style.port_activation_radius,
                )
                && let Some(node_id) = apply_connection(
                    view_graph,
                    func_lib,
                    self.connection_drag.start_port,
                    target.port,
                )
            {
                let port_idx = if target.port.kind == PortKind::Input {
                    target.port.idx
                } else {
                    self.connection_drag.start_port.idx
                };

                ui_interaction.actions.push((
                    node_id,
                    GraphUiAction::InputChanged {
                        input_idx: port_idx,
                    },
                ));
            }
            self.connection_drag.reset();
            self.state = InteractionState::Idle;
        }

        node_ui::render_nodes(&ctx, &graph_layout, view_graph, func_lib, ui_interaction);

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

        ui_interaction
            .actions
            .iter()
            .for_each(|action| match action.1 {
                GraphUiAction::CacheToggled => {}
                GraphUiAction::InputChanged { .. } => {}
                GraphUiAction::NodeRemoved => {
                    view_graph.remove_node(action.0);
                }
                GraphUiAction::NodeSelected => view_graph.select_node(action.0),
            });
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
        output_port.idx < output_func.outputs.len(),
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
        input_port.idx < input_func.inputs.len(),
        "input index must be valid for input node"
    );
    input_node.inputs[input_port.idx].binding = Binding::Bind(PortAddress {
        target_id: output_port.node_id,
        port_idx: output_port.idx,
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

fn breaker_path_length(points: &[Pos2]) -> f32 {
    points
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum()
}
