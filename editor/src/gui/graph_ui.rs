use eframe::egui;
use egui::{Id, PointerButton, Pos2, Ui, Vec2};
use graph::graph::NodeId;
use graph::prelude::{Binding, FuncLib, PortAddress};

use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_drag::ConnectionDrag;
use crate::gui::connection_ui::{ConnectionUi, PortKind};
use crate::gui::graph_layout::{GraphLayout, PortRef};
use crate::gui::node_ui::{NodeUi, PortDragInfo};
use crate::{
    gui::{graph_ctx::GraphContext, node_ui},
    model,
};

const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;
const MAX_BREAKER_LENGTH: f32 = 900.0;
const SCROLL_ZOOM_SPEED: f32 = 0.0015;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum InteractionState {
    #[default]
    Idle,
    BreakingConnections,
    DraggingNewConnection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrimaryState {
    Pressed,
    Down,
    Released,
}

#[derive(Debug, Default)]
pub struct GraphUi {
    state: InteractionState,
    graph_layout: GraphLayout,
    connection_breaker: ConnectionBreaker,
    connection_drag: Option<ConnectionDrag>,
    connection_renderer: ConnectionUi,
    node_ui: NodeUi,
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
        self.graph_layout = GraphLayout::default();
        self.connection_breaker.reset();
        self.connection_drag = None;
        self.connection_renderer = ConnectionUi::default();
    }

    pub fn render(
        &mut self,
        ui: &mut Ui,
        view_graph: &mut model::ViewGraph,
        func_lib: &FuncLib,
        ui_interaction: &mut GraphUiInteraction,
    ) {
        let mut ctx = GraphContext::new(ui, view_graph, func_lib);

        background(&ctx);

        let graph_bg_id = ctx.ui.make_persistent_id("graph_bg");

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

        if let Some(pointer_pos) = pointer_pos {
            update_zoom_and_pan(&mut ctx, graph_bg_id, pointer_pos);
        }

        self.graph_layout.update(&ctx);

        if let Some(pointer_pos) = pointer_pos {
            self.process_connections(&mut ctx, graph_bg_id, ui_interaction, pointer_pos);
        }

        self.render_connections(&mut ctx);

        let drag_port_info =
            self.node_ui
                .render_nodes(&mut ctx, &mut self.graph_layout, ui_interaction);

        self.process_connection_drag(&mut ctx, ui_interaction, drag_port_info);

        self.top_panel(&mut ctx);
    }

    fn process_connection_drag(
        &mut self,
        ctx: &mut GraphContext,
        ui_interaction: &mut GraphUiInteraction,
        drag_port_info: PortDragInfo,
    ) {
        match (self.state, drag_port_info) {
            (InteractionState::Idle, PortDragInfo::DragStart(port_info)) => {
                self.connection_drag = Some(ConnectionDrag::new(port_info));
                self.state = InteractionState::DraggingNewConnection;
            }
            (InteractionState::DraggingNewConnection, PortDragInfo::Hover(port_info)) => {
                let connection_drag = self.connection_drag.as_mut().unwrap();
                if connection_drag.start_port.port.kind != port_info.port.kind {
                    connection_drag.end_port = Some(port_info);
                    connection_drag.current_pos = port_info.center;
                }
            }
            (_, PortDragInfo::DragStop) => {
                let connection_drag = self.connection_drag.as_mut().unwrap();
                if let Some(end_port) = &connection_drag.end_port
                    && end_port.center.distance(connection_drag.current_pos)
                        < ctx.style.port_activation_radius
                {
                    let (input_node_id, input_idx) = apply_connection(
                        ctx.view_graph,
                        connection_drag.start_port.port,
                        end_port.port,
                    );
                    ui_interaction
                        .actions
                        .push((input_node_id, GraphUiAction::InputChanged { input_idx }));
                }
                self.connection_drag = None;
                self.state = InteractionState::Idle;
            }
            (_, _) => {}
        }
    }

    fn process_connections(
        &mut self,
        ctx: &mut GraphContext,
        graph_bg_id: Id,
        ui_interaction: &mut GraphUiInteraction,
        pointer_pos: Pos2,
    ) {
        let primary_state = ctx.ui.input(|input| {
            if input.pointer.primary_pressed() {
                Some(PrimaryState::Pressed)
            } else if input.pointer.primary_released() {
                Some(PrimaryState::Released)
            } else if input.pointer.primary_down() {
                Some(PrimaryState::Down)
            } else {
                None
            }
        });

        let pointer_on_background = ctx
            .ui
            .interact(ctx.rect, graph_bg_id, egui::Sense::hover())
            .hovered();

        match (self.state, primary_state) {
            (InteractionState::Idle, Some(PrimaryState::Pressed)) => {
                if pointer_on_background {
                    ctx.view_graph.selected_node_id = None;
                    self.state = InteractionState::BreakingConnections;
                    self.connection_breaker.reset();

                    self.connection_breaker.points.push(pointer_pos);
                }
            }
            (
                InteractionState::BreakingConnections,
                Some(PrimaryState::Pressed | PrimaryState::Down),
            ) => {
                let should_add = self
                    .connection_breaker
                    .points
                    .last()
                    .map(|last| last.distance(pointer_pos) > 2.0)
                    .unwrap_or(true);
                if should_add {
                    let remaining =
                        MAX_BREAKER_LENGTH - breaker_path_length(&self.connection_breaker.points);
                    let last_pos = self
                        .connection_breaker
                        .points
                        .last()
                        .copied()
                        .unwrap_or(pointer_pos);
                    let segment_len = last_pos.distance(pointer_pos);
                    if remaining > 0.0 && segment_len > 0.0 {
                        if segment_len <= remaining {
                            self.connection_breaker.points.push(pointer_pos);
                        } else {
                            let t = remaining / segment_len;
                            let clamped = Pos2::new(
                                last_pos.x + (pointer_pos.x - last_pos.x) * t,
                                last_pos.y + (pointer_pos.y - last_pos.y) * t,
                            );
                            self.connection_breaker.points.push(clamped);
                        }
                    }
                }
            }
            (InteractionState::BreakingConnections, _) => {
                for connection in self.connection_renderer.highlighted.iter() {
                    if let Some(node) = ctx
                        .view_graph
                        .graph
                        .nodes
                        .by_key_mut(&connection.input_node_id)
                    {
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
            (InteractionState::DraggingNewConnection, _) => {
                let drag = self.connection_drag.as_mut().unwrap();
                drag.current_pos = pointer_pos;
            }
            _ => {}
        }
    }

    fn render_connections(&mut self, ctx: &mut GraphContext) {
        self.connection_renderer.render(
            ctx,
            &self.graph_layout,
            ctx.view_graph,
            ctx.func_lib,
            if self.state == InteractionState::BreakingConnections {
                Some(&self.connection_breaker)
            } else {
                None
            },
        );

        match self.state {
            InteractionState::Idle => {}
            InteractionState::DraggingNewConnection => {
                if let Some(drag) = self.connection_drag.as_ref() {
                    drag.render(ctx, ctx.view_graph.scale);
                }
            }
            InteractionState::BreakingConnections => self.connection_breaker.render(ctx),
        }
    }

    fn top_panel(&self, ctx: &mut GraphContext) {
        let mut fit_all = false;
        let mut view_selected = false;

        ctx.ui.horizontal(|ui| {
            fit_all = ui.button("Fit all").clicked();
            view_selected = ui.button("View selected").clicked();
            let reset_view = ui.button("Reset view").clicked();
            if reset_view {
                ctx.view_graph.scale = 1.0;
                ctx.view_graph.pan = egui::Vec2::ZERO;
            }
        });
        if view_selected {
            view_selected_node(ctx, &self.graph_layout);
        }
        if fit_all {
            fit_all_nodes(ctx, &self.graph_layout);
        }
    }
}

fn update_zoom_and_pan(ctx: &mut GraphContext, graph_bg_id: Id, cursor_pos: Pos2) {
    let scroll_delta = ctx.ui.input(|input| input.smooth_scroll_delta);
    let pinch_delta = ctx.ui.input(|input| {
        if input.modifiers.command {
            1.0
        } else {
            input.zoom_delta()
        }
    });

    let zoom_delta = if scroll_delta.y.abs() > f32::EPSILON {
        (-scroll_delta.y * SCROLL_ZOOM_SPEED).exp() * pinch_delta
    } else {
        pinch_delta
    };
    let pan = if scroll_delta.x.abs() > f32::EPSILON {
        Vec2::new(scroll_delta.x, 0.0)
    } else {
        Vec2::ZERO
    };

    if (zoom_delta - 1.0).abs() > f32::EPSILON {
        let clamped_zoom = (ctx.view_graph.scale * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);

        if (clamped_zoom - ctx.view_graph.scale).abs() > f32::EPSILON {
            let origin = ctx.rect.min;
            let graph_pos = (cursor_pos - origin - ctx.view_graph.pan) / ctx.view_graph.scale;

            ctx.view_graph.scale = clamped_zoom;
            ctx.view_graph.pan = cursor_pos - origin - graph_pos * ctx.view_graph.scale;
        }
    }

    ctx.view_graph.pan += pan;

    let pan_response = ctx.ui.interact(ctx.rect, graph_bg_id, egui::Sense::drag());
    if pan_response.dragged_by(PointerButton::Middle) {
        ctx.view_graph.pan += pan_response.drag_delta();
    }
}

fn background(ctx: &GraphContext) {
    let spacing = ctx.style.dotted_base_spacing * ctx.view_graph.scale;
    let radius = (ctx.style.dotted_radius_base * ctx.view_graph.scale)
        .clamp(ctx.style.dotted_radius_min, ctx.style.dotted_radius_max);
    let color = ctx.style.dotted_color;
    let origin = ctx.rect.min + ctx.view_graph.pan;
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

// result -> (input_node_id, input_idx)
fn apply_connection(view_graph: &mut model::ViewGraph, a: PortRef, b: PortRef) -> (NodeId, usize) {
    let (input_port, output_port) = match (a.kind, b.kind) {
        (PortKind::Output, PortKind::Input) => (b, a),
        (PortKind::Input, PortKind::Output) => (a, b),
        _ => unreachable!("ports must be of opposite types"),
    };

    let input_node = view_graph.graph.by_id_mut(&input_port.node_id).unwrap();
    input_node.inputs[input_port.idx].binding = Binding::Bind(PortAddress {
        target_id: output_port.node_id,
        port_idx: output_port.idx,
    });

    (input_port.node_id, input_port.idx)
}

fn view_selected_node(ctx: &mut GraphContext, graph_layout: &GraphLayout) {
    let Some(selected_id) = ctx.view_graph.selected_node_id else {
        return;
    };
    let Some(node_view) = ctx.view_graph.view_nodes.by_key(&selected_id) else {
        return;
    };

    let node = ctx.view_graph.graph.by_id(&node_view.id).unwrap();
    let func = ctx.func_lib.by_id(&node.func_id).unwrap();

    let node_width = graph_layout.node_width(&node.id);
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
    ctx.view_graph.scale = 1.0;
    ctx.view_graph.pan = ctx.rect.center() - ctx.rect.min - center;
}

fn fit_all_nodes(ctx: &mut GraphContext, graph_layout: &GraphLayout) {
    if ctx.view_graph.view_nodes.is_empty() {
        ctx.view_graph.scale = 1.0;
        ctx.view_graph.pan = egui::Vec2::ZERO;
        return;
    }

    let mut min = Pos2::new(f32::INFINITY, f32::INFINITY);
    let mut max = Pos2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);

    for node_view in &ctx.view_graph.view_nodes {
        let node = ctx.view_graph.graph.by_id(&node_view.id).unwrap();
        let func = ctx.func_lib.by_id(&node.func_id).unwrap();
        let node_width = graph_layout.node_width(&node.id);
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
    ctx.view_graph.scale = target_zoom;

    let bounds_center = (min.to_vec2() + max.to_vec2()) * 0.5;
    ctx.view_graph.pan = ctx.rect.center() - ctx.rect.min - bounds_center * ctx.view_graph.scale;
}

fn breaker_path_length(points: &[Pos2]) -> f32 {
    points
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum()
}
