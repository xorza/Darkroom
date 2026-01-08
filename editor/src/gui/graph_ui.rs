use eframe::egui;
use egui::{Area, Id, PointerButton, Pos2, Response, Sense, Ui, Vec2};
use graph::graph::NodeId;
use graph::prelude::{Binding, ExecutionStats, FuncLib, PortAddress};

use crate::gui::background::BackgroundRenderer;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::PortKind;
use crate::gui::connection_ui::{ConnectionDragUpdate, ConnectionUi};
use crate::gui::graph_layout::{GraphLayout, PortRef};
use crate::gui::node_ui::{NodeUi, PortDragInfo};
use crate::{gui::graph_ctx::GraphContext, model};
use common::BoolExt;

const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;
const WHEEL_ZOOM_SPEED: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum InteractionState {
    #[default]
    Idle,
    BreakingConnections,
    DraggingNewConnection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerButtonState {
    Pressed,
    Down,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    CycleDetected {
        input_node_id: NodeId,
        output_node_id: NodeId,
    },
}

#[derive(Debug, Default)]
pub struct GraphUi {
    state: InteractionState,
    graph_layout: GraphLayout,
    connection_breaker: ConnectionBreaker,
    connections: ConnectionUi,
    node_ui: NodeUi,
    background: BackgroundRenderer,
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
        self.connections = ConnectionUi::default();
        self.background = BackgroundRenderer::default();
    }

    pub fn render(
        &mut self,
        ui: &mut Ui,
        view_graph: &mut model::ViewGraph,
        execution_stats: Option<&ExecutionStats>,
        func_lib: &FuncLib,
        ui_interaction: &mut GraphUiInteraction,
        arena: &bumpalo::Bump,
    ) -> Result<(), Error> {
        let mut ctx = GraphContext::new(arena, ui, func_lib, view_graph.scale);

        let graph_bg_id = ctx.ui.make_persistent_id("graph_bg");

        let pointer_pos = ctx
            .ui
            .input(|input| input.pointer.hover_pos())
            .and_then(|pos| ctx.rect.contains(pos).then_else(Some(pos), None));
        let background_response = ctx.ui.interact(
            ctx.rect,
            graph_bg_id,
            Sense::hover() | Sense::drag() | Sense::click(),
        );

        if background_response.clicked() {
            view_graph.selected_node_id = None;
        }

        if let Some(pointer_pos) = pointer_pos {
            self.update_zoom_and_pan(&mut ctx, view_graph, &background_response, pointer_pos);
        }

        self.graph_layout.update(&ctx, view_graph);

        self.background.render(&ctx, view_graph);

        self.render_connections(&mut ctx, view_graph);

        let drag_port_info = self.node_ui.render_nodes(
            &mut ctx,
            view_graph,
            &mut self.graph_layout,
            ui_interaction,
            execution_stats,
        );

        self.top_panel(&mut ctx, view_graph);

        if let Some(pointer_pos) = pointer_pos {
            self.process_connections(
                &mut ctx,
                view_graph,
                &background_response,
                ui_interaction,
                pointer_pos,
                drag_port_info,
            )?;
        }

        Ok(())
    }

    fn process_connections(
        &mut self,
        ctx: &mut GraphContext,
        view_graph: &mut model::ViewGraph,
        background_response: &Response,
        ui_interaction: &mut GraphUiInteraction,
        pointer_pos: Pos2,
        drag_port_info: PortDragInfo,
    ) -> Result<(), Error> {
        let primary_state = ctx.ui.input(|input| {
            if input.pointer.primary_pressed() {
                Some(PointerButtonState::Pressed)
            } else if input.pointer.primary_released() {
                Some(PointerButtonState::Released)
            } else if input.pointer.primary_down() {
                Some(PointerButtonState::Down)
            } else {
                None
            }
        });
        let secondary_pressed = ctx.ui.input(|input| input.pointer.secondary_pressed());

        let pointer_on_background = background_response.hovered();

        let primary_pressed = matches!(primary_state, Some(PointerButtonState::Pressed));
        let primary_down = matches!(
            primary_state,
            Some(PointerButtonState::Pressed | PointerButtonState::Down)
        );

        match self.state {
            InteractionState::Idle => {
                if primary_pressed {
                    if let PortDragInfo::DragStart(port_info) = drag_port_info {
                        self.connections.start_drag(port_info);
                        self.state = InteractionState::DraggingNewConnection;
                    } else if pointer_on_background {
                        self.state = InteractionState::BreakingConnections;
                        self.connection_breaker.start(pointer_pos);
                    }
                }
            }
            InteractionState::BreakingConnections => {
                if secondary_pressed {
                    self.connection_breaker.reset();
                    self.state = InteractionState::Idle;
                } else if primary_down {
                    self.connection_breaker.add_point(pointer_pos);
                } else {
                    for connection in self.connections.highlighted.iter() {
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
            }
            InteractionState::DraggingNewConnection => {
                let update = self.connections.update_drag(pointer_pos, drag_port_info);
                match update {
                    ConnectionDragUpdate::InProgress => {}
                    ConnectionDragUpdate::Finished => self.state = InteractionState::Idle,
                    ConnectionDragUpdate::FinishedWith {
                        start_port,
                        end_port,
                    } => {
                        self.state = InteractionState::Idle;

                        let (input_node_id, input_idx) =
                            apply_connection(view_graph, start_port.port, end_port.port)?;
                        ui_interaction
                            .actions
                            .push((input_node_id, GraphUiAction::InputChanged { input_idx }));
                    }
                }
            }
        }

        Ok(())
    }

    fn render_connections(&mut self, ctx: &mut GraphContext, view_graph: &model::ViewGraph) {
        self.connections.render(
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
            InteractionState::DraggingNewConnection => {}
            InteractionState::BreakingConnections => self.connection_breaker.render(ctx),
        }
    }

    fn top_panel(&self, ctx: &mut GraphContext, view_graph: &mut model::ViewGraph) {
        let mut fit_all = false;
        let mut view_selected = false;
        let mut reset_view = false;

        let panel_pos = ctx.rect.min + Vec2::splat(ctx.style.padding);
        Area::new(Id::new("graph_top_buttons"))
            .fixed_pos(panel_pos)
            .show(ctx.ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    fit_all = ui.button("Fit all").clicked();
                    view_selected = ui.button("View selected").clicked();
                    reset_view = ui.button("Reset view").clicked();
                });
            });

        if reset_view {
            view_graph.scale = 1.0;
            view_graph.pan = Vec2::ZERO;
        }
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
        background_response: &Response,
        pointer_pos: Pos2,
    ) {
        let (zoom_delta, pan) = {
            let (scroll_delta, mouse_wheel_delta) = collect_scroll_mouse_wheel_deltas(ctx);

            (mouse_wheel_delta.abs() > f32::EPSILON).then_else(
                ((mouse_wheel_delta * WHEEL_ZOOM_SPEED).exp(), Vec2::ZERO),
                (
                    ctx.ui
                        .input(|input| input.modifiers.command.then_else(1.0, input.zoom_delta())),
                    scroll_delta,
                ),
            )
        };

        // if (zoom_delta - 1.0).abs() > 0.001 || pan.length() > 0.001 {
        //     println!("zoom_delta: {}, pan {}", zoom_delta, pan);
        // }

        if (zoom_delta - 1.0).abs() > f32::EPSILON {
            // zoom
            let clamped_scale = (view_graph.scale * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
            let origin = ctx.rect.min;
            let graph_pos = (pointer_pos - origin - view_graph.pan) / view_graph.scale;
            view_graph.scale = clamped_scale;
            ctx.scale = clamped_scale;
            view_graph.pan = pointer_pos - origin - graph_pos * view_graph.scale;
        }

        view_graph.pan += pan;

        if background_response.dragged_by(PointerButton::Middle) {
            view_graph.pan += background_response.drag_delta();
        }
    }
}

/// Returns smooth scroll delta plus an accumulated mouse-wheel line/page magnitude.
///
/// Trackpad/gesture scrolling is folded into the returned `Vec2`, while mouse wheel
/// steps (line/page units) are accumulated separately to keep zoom/pan heuristics stable.
fn collect_scroll_mouse_wheel_deltas(ctx: &mut GraphContext<'_>) -> (Vec2, f32) {
    let (scroll_delta, mouse_wheel_delta) = {
        let base_scroll_delta = ctx.ui.input(|input| input.raw_scroll_delta);
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
                            (point, event_delta.y)
                        }
                    },
                    _ => (point, lines),
                },
            )
        })
    };
    (scroll_delta, mouse_wheel_delta)
}

/// Connects an output port to an input port in `view_graph`.
///
/// Returns the input node id and input port index that were updated, or a cycle error if the
/// connection would introduce a loop.
///
/// # Panics
/// Panics if the ports are not of opposite kinds, or if the input node id
/// is not present in the graph.
fn apply_connection(
    view_graph: &mut model::ViewGraph,
    a: PortRef,
    b: PortRef,
) -> Result<(NodeId, usize), Error> {
    let (input_port, output_port) = match (a.kind, b.kind) {
        (PortKind::Output, PortKind::Input) => (b, a),
        (PortKind::Input, PortKind::Output) => (a, b),
        _ => unreachable!("ports must be of opposite types"),
    };

    if input_port.node_id == output_port.node_id {
        return Err(Error::CycleDetected {
            input_node_id: input_port.node_id,
            output_node_id: output_port.node_id,
        });
    }

    let dependents = view_graph.graph.dependent_nodes(&input_port.node_id);
    if dependents.contains(&output_port.node_id) {
        return Err(Error::CycleDetected {
            input_node_id: input_port.node_id,
            output_node_id: output_port.node_id,
        });
    }

    let input_node = view_graph.graph.by_id_mut(&input_port.node_id).unwrap();
    input_node.inputs[input_port.idx].binding = Binding::Bind(PortAddress {
        target_id: output_port.node_id,
        port_idx: output_port.idx,
    });

    Ok((input_port.node_id, input_port.idx))
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

    let scale = view_graph.scale;
    let rect = graph_layout.node_layout(&node_view.id).body_rect;
    let size = rect.size() / scale;
    let center = egui::pos2(
        node_view.pos.x + size.x * 0.5,
        node_view.pos.y + size.y * 0.5,
    );
    view_graph.scale = 1.0;
    view_graph.pan = ctx.rect.center() - ctx.rect.min - center.to_vec2();
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

    let mut layouts = graph_layout.node_layouts.iter();
    let first = layouts.next().unwrap();
    let mut bounds = to_graph_rect(first.body_rect);
    for layout in layouts {
        bounds = bounds.union(to_graph_rect(layout.body_rect));
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

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::CycleDetected {
                input_node_id,
                output_node_id,
            } => write!(
                f,
                "connection would create a cycle between {} and {}",
                input_node_id, output_node_id
            ),
        }
    }
}
