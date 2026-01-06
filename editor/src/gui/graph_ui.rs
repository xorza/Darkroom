use eframe::egui;
use egui::{Id, PointerButton, Pos2, Ui, Vec2};
use graph::graph::NodeId;
use graph::prelude::{Binding, FuncLib, PortAddress};

use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_drag::ConnectionDrag;
use crate::gui::connection_ui::{ConnectionUi, PortKind};
use crate::gui::graph_layout::{GraphLayout, PortRef};
use crate::gui::node_ui::{NodeUi, PortDragInfo};
use crate::{gui::graph_ctx::GraphContext, model};
use common::BoolExt;

const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;
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
        let mut ctx = GraphContext::new(ui, func_lib);

        background(&ctx, view_graph);

        let graph_bg_id = ctx.ui.make_persistent_id("graph_bg");

        let pointer_pos = ctx
            .ui
            .input(|input| input.pointer.hover_pos())
            .and_then(|pos| ctx.rect.contains(pos).then_else(Some(pos), None));

        if let Some(pointer_pos) = pointer_pos {
            self.update_zoom_and_pan(&mut ctx, view_graph, graph_bg_id, pointer_pos);
        }

        self.graph_layout.update(&ctx, view_graph);

        let drag_port_info = self.node_ui.process_input(
            &mut ctx,
            view_graph,
            &mut self.graph_layout,
            ui_interaction,
        );

        if let Some(pointer_pos) = pointer_pos {
            self.process_connections(
                &mut ctx,
                view_graph,
                graph_bg_id,
                ui_interaction,
                pointer_pos,
                drag_port_info,
            );
        }

        self.render_connections(&mut ctx, view_graph);

        self.node_ui
            .render_nodes(&mut ctx, view_graph, &mut self.graph_layout);

        self.top_panel(&mut ctx, view_graph);
    }

    fn process_connections(
        &mut self,
        ctx: &mut GraphContext,
        view_graph: &mut model::ViewGraph,
        graph_bg_id: Id,
        ui_interaction: &mut GraphUiInteraction,
        pointer_pos: Pos2,
        drag_port_info: PortDragInfo,
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

        let primary_pressed = matches!(primary_state, Some(PrimaryState::Pressed));
        let primary_down = matches!(
            primary_state,
            Some(PrimaryState::Pressed | PrimaryState::Down)
        );

        match self.state {
            InteractionState::Idle => {
                if primary_pressed {
                    if let PortDragInfo::DragStart(port_info) = drag_port_info {
                        self.connection_drag = Some(ConnectionDrag::new(port_info));
                        self.state = InteractionState::DraggingNewConnection;
                    } else if pointer_on_background {
                        view_graph.selected_node_id = None;
                        self.state = InteractionState::BreakingConnections;
                        self.connection_breaker.reset();
                        self.connection_breaker.points.push(pointer_pos);
                    }
                }
            }
            InteractionState::BreakingConnections => {
                if primary_down {
                    self.connection_breaker.add_point(pointer_pos);
                    return;
                }

                for connection in self.connection_renderer.highlighted.iter() {
                    let node = view_graph
                        .graph
                        .nodes
                        .by_key_mut(&connection.input_node_id)
                        .unwrap();

                    node.inputs[connection.input_idx].binding = Binding::None;
                    ui_interaction.actions.push((
                        connection.input_node_id,
                        GraphUiAction::InputChanged {
                            input_idx: connection.input_idx,
                        },
                    ));
                }
                self.connection_breaker.reset();
                self.state = InteractionState::Idle;
            }
            InteractionState::DraggingNewConnection => {
                let connection_drag = self.connection_drag.as_mut().unwrap();
                connection_drag.current_pos = pointer_pos;

                match drag_port_info {
                    PortDragInfo::Hover(port_info) => {
                        if connection_drag.start_port.port.kind != port_info.port.kind {
                            connection_drag.end_port = Some(port_info);
                            connection_drag.current_pos = port_info.center;
                        }
                    }
                    PortDragInfo::DragStop => {
                        if let Some(end_port) = &connection_drag.end_port
                            && end_port.center.distance(connection_drag.current_pos)
                                < ctx.style.port_activation_radius
                        {
                            let (input_node_id, input_idx) = apply_connection(
                                view_graph,
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
                    PortDragInfo::DragStart(_) | PortDragInfo::None => {}
                }
            }
        }
    }

    fn render_connections(&mut self, ctx: &mut GraphContext, view_graph: &model::ViewGraph) {
        self.connection_renderer.render(
            ctx,
            &self.graph_layout,
            view_graph,
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
                    drag.render(ctx, view_graph.scale);
                }
            }
            InteractionState::BreakingConnections => self.connection_breaker.render(ctx),
        }
    }

    fn top_panel(&self, ctx: &mut GraphContext, view_graph: &mut model::ViewGraph) {
        let mut fit_all = false;
        let mut view_selected = false;

        ctx.ui.horizontal(|ui| {
            fit_all = ui.button("Fit all").clicked();
            view_selected = ui.button("View selected").clicked();
            let reset_view = ui.button("Reset view").clicked();
            if reset_view {
                view_graph.scale = 1.0;
                view_graph.pan = egui::Vec2::ZERO;
            }
        });
        if view_selected {
            view_selected_node(ctx, view_graph, &self.graph_layout);
        }
        if fit_all {
            fit_all_nodes(ctx, view_graph, &self.graph_layout);
        }
    }

    fn update_zoom_and_pan(
        &mut self,
        ctx: &mut GraphContext,
        view_graph: &mut model::ViewGraph,
        graph_bg_id: Id,
        cursor_pos: Pos2,
    ) {
        let (scroll_delta, mouse_wheel_delta) = collect_scroll_mouse_wheel_deltas(ctx);
        let (zoom_delta, pan) = (mouse_wheel_delta > f32::EPSILON).then_else(
            ((-mouse_wheel_delta * SCROLL_ZOOM_SPEED).exp(), Vec2::ZERO),
            (
                ctx.ui
                    .input(|input| input.modifiers.command.then_else(1.0, input.zoom_delta())),
                scroll_delta,
            ),
        );

        {
            // zoom
            let clamped_zoom = (view_graph.scale * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
            let origin = ctx.rect.min;
            let graph_pos = (cursor_pos - origin - view_graph.pan) / view_graph.scale;
            view_graph.scale = clamped_zoom;
            view_graph.pan = cursor_pos - origin - graph_pos * view_graph.scale;
        }

        view_graph.pan += pan;

        let pan_response = ctx.ui.interact(ctx.rect, graph_bg_id, egui::Sense::drag());
        if pan_response.dragged_by(PointerButton::Middle) {
            view_graph.pan += pan_response.drag_delta();
        }
    }
}

fn collect_scroll_mouse_wheel_deltas(ctx: &mut GraphContext<'_>) -> (Vec2, f32) {
    let (scroll_delta, mouse_wheel_delta) = {
        let base_scroll_delta = ctx.ui.input(|input| input.smooth_scroll_delta);
        ctx.ui.input(|input| {
            input.events.iter().fold(
                (base_scroll_delta, 0.0),
                |(point, lines), event| match event {
                    egui::Event::MouseWheel {
                        unit,
                        delta: event_delta,
                        ..
                    } => match unit {
                        egui::MouseWheelUnit::Point => (point + *event_delta, lines),
                        egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => {
                            (point, lines + event_delta.length())
                        }
                    },
                    _ => (point, lines),
                },
            )
        })
    };
    (scroll_delta, mouse_wheel_delta)
}

fn background(ctx: &GraphContext, view_graph: &model::ViewGraph) {
    let spacing = ctx.style.dotted_base_spacing * view_graph.scale;
    let radius = (ctx.style.dotted_radius_base * view_graph.scale)
        .clamp(ctx.style.dotted_radius_min, ctx.style.dotted_radius_max);
    let color = ctx.style.dotted_color;
    let origin = ctx.rect.min + view_graph.pan;
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

/// Connects an output port to an input port in `view_graph`.
///
/// Returns the input node id and input port index that were updated.
///
/// # Panics
/// Panics if the ports are not of opposite kinds, or if the input node id
/// is not present in the graph.
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

fn view_selected_node(
    ctx: &mut GraphContext,
    view_graph: &mut model::ViewGraph,
    graph_layout: &GraphLayout,
) {
    let Some(selected_id) = view_graph.selected_node_id else {
        return;
    };
    let Some(node_view) = view_graph.view_nodes.by_key(&selected_id) else {
        return;
    };

    let rect = graph_layout.node_rect(&node_view.id);
    view_graph.scale = 1.0;
    view_graph.pan = ctx.rect.center() - ctx.rect.min - rect.center().to_vec2();
}

fn fit_all_nodes(
    ctx: &mut GraphContext,
    view_graph: &mut model::ViewGraph,
    graph_layout: &GraphLayout,
) {
    if view_graph.view_nodes.is_empty() {
        view_graph.scale = 1.0;
        view_graph.pan = egui::Vec2::ZERO;
        return;
    }

    let origin = graph_layout.origin;
    let scale = view_graph.scale;
    let to_graph_rect = |rect: egui::Rect| {
        let min = (rect.min - origin) / scale;
        let max = (rect.max - origin) / scale;
        egui::Rect::from_min_max(egui::pos2(min.x, min.y), egui::pos2(max.x, max.y))
    };

    let mut layouts = graph_layout.node_layouts.values();
    let first = layouts.next().unwrap();
    let mut bounds = to_graph_rect(first.rect);
    for layout in layouts {
        bounds = bounds.union(to_graph_rect(layout.rect));
    }

    let bounds_size = bounds.size();

    let padding = 24.0;
    let available = ctx.rect.size() - egui::vec2(padding * 2.0, padding * 2.0);
    let zoom_x = (bounds_size.x > 0.0).then_else(available.x / bounds_size.x, 1.0);
    let zoom_y = (bounds_size.y > 0.0).then_else(available.y / bounds_size.y, 1.0);

    let target_zoom = zoom_x.min(zoom_y).clamp(MIN_ZOOM, MAX_ZOOM);
    view_graph.scale = target_zoom;
    let bounds_center = bounds.center().to_vec2();
    view_graph.pan = ctx.rect.center() - ctx.rect.min - bounds_center * view_graph.scale;
}
