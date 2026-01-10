use std::ptr::NonNull;

use eframe::egui;
use egui::{
    Area, Button, Color32, Frame, Id, Key, Margin, PointerButton, Pos2, Response, RichText, Sense,
    StrokeKind, Vec2,
};
use graph::graph::NodeId;
use graph::prelude::{Binding, ExecutionStats, FuncLib, PortAddress};

use crate::gui::background::DottedBackgroundRenderer;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::PortKind;
use crate::gui::connection_ui::{ConnectionDragUpdate, ConnectionUi};
use crate::gui::graph_layout::{GraphLayout, PortRef};
use crate::gui::node_ui::{NodeUi, PortDragInfo};
use crate::{gui::Gui, gui::graph_ctx::GraphContext, model};
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
    dots_background: DottedBackgroundRenderer,
    interaction: GraphUiInteraction,
}

#[derive(Debug, Default)]
pub(crate) struct GraphUiInteraction {
    pub actions: Vec<GraphUiAction>,
    pub errors: Vec<Error>,
    pub run: bool,
}

impl GraphUiInteraction {
    pub fn clear(&mut self) {
        self.actions.clear();
        self.errors.clear();
        self.run = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphUiAction {
    CacheToggled { node_id: NodeId },
    InputChanged { node_id: NodeId, input_idx: usize },
    NodeRemoved { node_id: NodeId },
    NodeSelected { node_id: Option<NodeId> },
    ZoomPanChanged,
}

impl GraphUi {
    pub fn reset(&mut self) {
        self.state = InteractionState::Idle;
        self.graph_layout = GraphLayout::default();
        self.connection_breaker.reset();
        self.connections = ConnectionUi::default();
        self.dots_background = DottedBackgroundRenderer::default();
        self.interaction = GraphUiInteraction::default();
    }

    pub fn render(
        &mut self,
        gui: &mut Gui<'_>,
        view_graph: &mut model::ViewGraph,
        execution_stats: Option<&ExecutionStats>,
        func_lib: &FuncLib,
    ) -> &GraphUiInteraction {
        self.interaction.clear();
        let rect = gui.ui().available_rect_before_wrap();
        let rect = rect.shrink(gui.style.big_padding);

        gui.painter().rect(
            rect,
            gui.style.corner_radius,
            gui.style.graph_background.bg_color,
            gui.style.inactive_bg_stroke,
            StrokeKind::Outside,
        );

        let rect = rect.shrink(gui.style.corner_radius * 0.5);
        let mut graph_ui = gui
            .ui()
            .new_child(egui::UiBuilder::new().id_salt("graph_ui").max_rect(rect));

        let mut gui = Gui::new(&mut graph_ui, gui.style.clone());
        gui.ui().set_clip_rect(rect);

        let mut ctx = GraphContext::new(func_lib, view_graph, execution_stats);

        let graph_bg_id = gui.ui().make_persistent_id("graph_bg");

        let rect = gui.rect;
        let pointer_pos = gui
            .ui()
            .input(|input| input.pointer.hover_pos())
            .and_then(|pos| rect.contains(pos).then_else(Some(pos), None));
        let background_response = gui.ui().interact(
            rect,
            graph_bg_id,
            Sense::hover() | Sense::drag() | Sense::click(),
        );

        if background_response.clicked() && ctx.view_graph.selected_node_id.is_some() {
            ctx.view_graph.selected_node_id = None;
            self.interaction
                .actions
                .push(GraphUiAction::NodeSelected { node_id: None });
        }

        self.top_panel(&mut gui, &mut ctx);

        if let Some(pointer_pos) = pointer_pos {
            self.update_zoom_and_pan(&mut gui, &mut ctx, &background_response, pointer_pos);
        }

        gui.set_scale(ctx.view_graph.scale);
        self.graph_layout.update(&gui, &ctx);
        self.dots_background.render(&gui, &ctx);
        self.render_connections(&mut gui, &mut ctx);

        let drag_port_info = self.node_ui.render_nodes(
            &mut gui,
            &mut ctx,
            &mut self.graph_layout,
            &mut self.interaction,
            if self.state == InteractionState::BreakingConnections {
                Some(&self.connection_breaker)
            } else {
                None
            },
        );

        if let Some(pointer_pos) = pointer_pos {
            self.process_connections(
                &mut gui,
                &mut ctx,
                &background_response,
                pointer_pos,
                drag_port_info,
            );
        }

        gui.set_scale(1.0);

        &self.interaction
    }

    #[allow(clippy::too_many_arguments)]
    fn process_connections(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        background_response: &Response,
        pointer_pos: Pos2,
        drag_port_info: PortDragInfo,
    ) {
        let graph_ui_interaction = &mut self.interaction;
        let primary_state = gui.ui().input(|input| {
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
        let secondary_pressed = gui.ui().input(|input| input.pointer.secondary_pressed());

        let pointer_on_background =
            background_response.hovered() && !self.connections.any_hovered();

        let primary_pressed = matches!(primary_state, Some(PointerButtonState::Pressed));
        let primary_down = matches!(
            primary_state,
            Some(PointerButtonState::Pressed | PointerButtonState::Down)
        );

        let esc_pressed = gui.ui().input(|input| input.key_pressed(Key::Escape));

        if secondary_pressed || esc_pressed {
            self.connection_breaker.reset();
            self.state = InteractionState::Idle;
            self.connections.stop_drag();
            return;
        }

        match self.state {
            InteractionState::Idle => {
                if primary_pressed {
                    if let PortDragInfo::DragStart(_) = drag_port_info {
                        self.connections.update_drag(pointer_pos, drag_port_info);
                        self.state = InteractionState::DraggingNewConnection;
                    } else if pointer_on_background {
                        self.state = InteractionState::BreakingConnections;
                        self.connection_breaker.start(pointer_pos);
                    }
                }
            }
            InteractionState::BreakingConnections => {
                if primary_down {
                    self.connection_breaker.add_point(pointer_pos);
                } else {
                    self.connection_breaker.reset();
                    self.state = InteractionState::Idle;

                    let iter = self
                        .connections
                        .broke_iter()
                        .chain(self.node_ui.const_bind_ui.broke_iter());

                    for connection in iter {
                        let node = ctx
                            .view_graph
                            .graph
                            .nodes
                            .by_key_mut(&connection.input_node_id)
                            .unwrap();

                        node.inputs[connection.input_idx].binding = Binding::None;
                        graph_ui_interaction
                            .actions
                            .push(GraphUiAction::InputChanged {
                                node_id: connection.input_node_id,
                                input_idx: connection.input_idx,
                            });
                    }

                    for node_id in self.node_ui.node_ids_hit_breaker.iter() {
                        ctx.view_graph.remove_node(node_id);

                        graph_ui_interaction
                            .actions
                            .push(GraphUiAction::NodeRemoved { node_id: *node_id });
                    }
                }
            }
            InteractionState::DraggingNewConnection => {
                let update = self.connections.update_drag(pointer_pos, drag_port_info);
                match update {
                    ConnectionDragUpdate::InProgress => {}
                    ConnectionDragUpdate::Finished => {
                        self.state = InteractionState::Idle;
                    }
                    ConnectionDragUpdate::FinishedWithEmptyOutput { input_port } => {
                        assert_eq!(input_port.kind, PortKind::Input);

                        self.state = InteractionState::Idle;

                        let input_node =
                            ctx.view_graph.graph.by_id_mut(&input_port.node_id).unwrap();
                        input_node.inputs[input_port.port_idx].binding = Binding::Const(0.into());
                    }
                    ConnectionDragUpdate::FinishedWith {
                        input_port,
                        output_port,
                    } => {
                        assert_eq!(input_port.kind, PortKind::Input);
                        assert_eq!(output_port.kind, PortKind::Output);

                        self.state = InteractionState::Idle;

                        let result = apply_connection(ctx.view_graph, input_port, output_port);
                        match result {
                            Ok((input_node_id, input_idx)) => {
                                graph_ui_interaction
                                    .actions
                                    .push(GraphUiAction::InputChanged {
                                        node_id: input_node_id,
                                        input_idx,
                                    })
                            }
                            Err(err) => graph_ui_interaction.errors.push(err),
                        }
                    }
                }
            }
        }
    }

    fn render_connections(&mut self, gui: &mut Gui<'_>, ctx: &mut GraphContext<'_>) {
        let ui_interaction = &mut self.interaction;
        self.connections.render(
            gui,
            ctx,
            &self.graph_layout,
            ui_interaction,
            if self.state == InteractionState::BreakingConnections {
                Some(&self.connection_breaker)
            } else {
                None
            },
        );

        match self.state {
            InteractionState::Idle => {}
            InteractionState::DraggingNewConnection => {}
            InteractionState::BreakingConnections => self.connection_breaker.show(gui),
        }
    }

    fn top_panel(&mut self, gui: &mut Gui<'_>, ctx: &mut GraphContext) {
        let graph_ui_interaction = &mut self.interaction;
        let mut fit_all = false;
        let mut view_selected = false;
        let mut reset_view = false;

        let panel_pos = gui.rect.min;
        let panel_width = gui.rect.width();
        let panel_rect = egui::Rect::from_min_size(panel_pos, Vec2::new(panel_width, 0.0));
        let mono_font = gui.style.mono_font.clone();
        let small_padding = gui.style.small_padding;
        let button_size = mono_font.size + small_padding * 2.0;

        assert!(button_size.is_finite());
        assert!(button_size > 0.0);
        let button_size = Vec2::splat(button_size);

        Area::new(Id::new("graph_top_buttons"))
            .fixed_pos(panel_pos)
            .show(gui.ui().ctx(), |ui| {
                ui.scope_builder(egui::UiBuilder::new().max_rect(panel_rect), |ui| {
                    ui.set_min_width(panel_width);
                    ui.set_max_width(panel_width);
                    let frame = Frame::NONE
                        .fill(Color32::from_black_alpha(64))
                        .inner_margin(small_padding);
                    frame.show(ui, |ui| {
                        ui.set_min_width(panel_width - small_padding * 2.0);
                        ui.set_max_width(panel_width);

                        ui.horizontal(|ui| {
                            let mut make_button = |label| {
                                ui.add_sized(
                                    button_size,
                                    Button::new(RichText::new(label).font(mono_font.clone())),
                                )
                                .clicked()
                            };
                            graph_ui_interaction.run |= make_button("run");
                            fit_all = make_button("a");
                            view_selected = make_button("s");
                            reset_view = make_button("r");
                        });
                    });
                });
            });

        if reset_view {
            ctx.view_graph.scale = 1.0;
            gui.set_scale(ctx.view_graph.scale);
            ctx.view_graph.pan = Vec2::ZERO;
        }
        if view_selected {
            view_selected_node(gui, ctx, &self.graph_layout);
        }
        if fit_all {
            fit_all_nodes(gui, ctx, &self.graph_layout);
        }
    }

    fn update_zoom_and_pan(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        background_response: &Response,
        pointer_pos: Pos2,
    ) {
        let prev_scale = ctx.view_graph.scale;
        let prev_pan = ctx.view_graph.pan;
        let (zoom_delta, pan) = {
            let (scroll_delta, mouse_wheel_delta) = collect_scroll_mouse_wheel_deltas(gui);

            (mouse_wheel_delta.abs() > f32::EPSILON).then_else(
                ((mouse_wheel_delta * WHEEL_ZOOM_SPEED).exp(), Vec2::ZERO),
                (
                    gui.ui()
                        .input(|input| input.modifiers.command.then_else(1.0, input.zoom_delta())),
                    scroll_delta,
                ),
            )
        };

        if (zoom_delta - 1.0).abs() > f32::EPSILON {
            // zoom
            let clamped_scale = (ctx.view_graph.scale * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
            let origin = gui.rect.min;
            let graph_pos = (pointer_pos - origin - ctx.view_graph.pan) / ctx.view_graph.scale;
            ctx.view_graph.scale = clamped_scale;
            ctx.view_graph.pan = pointer_pos - origin - graph_pos * ctx.view_graph.scale;
        }

        ctx.view_graph.pan += pan;

        if background_response.dragged_by(PointerButton::Middle) {
            ctx.view_graph.pan += background_response.drag_delta();
        }

        if crate::common::scale_changed(prev_scale, ctx.view_graph.scale)
            || crate::common::pan_changed_v2(prev_pan, ctx.view_graph.pan)
        {
            // todo only when stopped panning and zooming
            // self.interaction.actions.push(GraphUiAction::ZoomPanChanged);
        }
    }
}

/// Returns smooth scroll delta plus an accumulated mouse-wheel line/page magnitude.
///
/// Trackpad/gesture scrolling is folded into the returned `Vec2`, while mouse wheel
/// steps (line/page units) are accumulated separately to keep zoom/pan heuristics stable.
fn collect_scroll_mouse_wheel_deltas(gui: &mut Gui<'_>) -> (Vec2, f32) {
    let (scroll_delta, mouse_wheel_delta) = {
        let base_scroll_delta = gui.ui().input(|input| input.raw_scroll_delta);
        gui.ui().input(|input| {
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
    input_port: PortRef,
    output_port: PortRef,
) -> Result<(NodeId, usize), Error> {
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
    input_node.inputs[input_port.port_idx].binding = Binding::Bind(PortAddress {
        target_id: output_port.node_id,
        port_idx: output_port.port_idx,
    });

    Ok((input_port.node_id, input_port.port_idx))
}

fn view_selected_node(gui: &mut Gui<'_>, ctx: &mut GraphContext<'_>, graph_layout: &GraphLayout) {
    let Some(selected_id) = ctx.view_graph.selected_node_id else {
        return;
    };
    let Some(node_view) = ctx.view_graph.view_nodes.by_key(&selected_id) else {
        return;
    };

    let scale = ctx.view_graph.scale;
    let rect = graph_layout.node_layout(&node_view.id).body_rect;
    let size = rect.size() / scale;
    let center = egui::pos2(
        node_view.pos.x + size.x * 0.5,
        node_view.pos.y + size.y * 0.5,
    );
    ctx.view_graph.scale = 1.0;
    gui.set_scale(ctx.view_graph.scale);
    ctx.view_graph.pan = gui.rect.center() - gui.rect.min - center.to_vec2();
}

fn fit_all_nodes(gui: &mut Gui<'_>, ctx: &mut GraphContext<'_>, graph_layout: &GraphLayout) {
    if ctx.view_graph.view_nodes.is_empty() {
        ctx.view_graph.scale = 1.0;
        gui.set_scale(ctx.view_graph.scale);
        ctx.view_graph.pan = egui::Vec2::ZERO;
        return;
    }

    let origin = graph_layout.origin;
    let scale = ctx.view_graph.scale;
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
    let available = gui.rect.size() - egui::vec2(padding * 2.0, padding * 2.0);
    let zoom_x = (bounds_size.x > 0.0).then_else(available.x / bounds_size.x, 1.0);
    let zoom_y = (bounds_size.y > 0.0).then_else(available.y / bounds_size.y, 1.0);

    let target_zoom = zoom_x.min(zoom_y).clamp(MIN_ZOOM, MAX_ZOOM);
    ctx.view_graph.scale = target_zoom;
    gui.set_scale(ctx.view_graph.scale);
    let bounds_center = bounds.center().to_vec2();
    ctx.view_graph.pan = gui.rect.center() - gui.rect.min - bounds_center * ctx.view_graph.scale;
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
