use std::ptr::NonNull;

use eframe::egui;
use egui::{
    Align, Align2, Area, Color32, FontId, Frame, Id, Key, Layout, Margin, PointerButton, Pos2,
    Rect, Response, RichText, Sense, Shape, StrokeKind, UiBuilder, Vec2, pos2, vec2,
};
use graph::graph::NodeId;
use graph::prelude::{Binding, ExecutionStats, FuncLib, PortAddress};

use crate::common::UiEquals;
use crate::common::button;
use crate::common::toggle_button::ToggleButton;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::connection_ui::{ConnectionDragUpdate, ConnectionUi};
use crate::gui::connection_ui::{ConnectionKey, PortKind};
use crate::gui::graph_background::GraphBackgroundRenderer;
use crate::gui::graph_layout::{GraphLayout, PortRef};
use crate::gui::graph_ui_interaction::{
    AutorunCommand, EventSubscriberChange, GraphUiAction, GraphUiInteraction,
};
use crate::gui::node_ui::{NodeUi, PortDragInfo};
use crate::{gui::Gui, gui::graph_ctx::GraphContext, model};
use common::BoolExt;

const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;
const WHEEL_ZOOM_SPEED: f32 = 0.08;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum InteractionState {
    #[default]
    Idle,
    BreakingConnections,
    DraggingNewConnection,
    PanningGraph,
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
    dots_background: GraphBackgroundRenderer,
    autorun_enabled: bool,
}

impl GraphUi {
    pub fn render(
        &mut self,
        gui: &mut Gui<'_>,
        view_graph: &mut model::ViewGraph,
        execution_stats: Option<&ExecutionStats>,
        func_lib: &FuncLib,
        interaction: &mut GraphUiInteraction,
    ) {
        let rect = gui
            .ui()
            .available_rect_before_wrap()
            .shrink(gui.style.big_padding);

        gui.painter().rect(
            rect,
            gui.style.corner_radius,
            gui.style.graph_background.bg_color,
            gui.style.inactive_bg_stroke,
            StrokeKind::Inside,
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
            let before = ctx.view_graph.selected_node_id;
            ctx.view_graph.selected_node_id = None;
            interaction.add_action(GraphUiAction::NodeSelected {
                before,
                after: None,
            });
        }

        self.update_zoom_and_pan(
            &mut gui,
            &mut ctx,
            &background_response,
            pointer_pos,
            interaction,
        );

        gui.set_scale(ctx.view_graph.scale);
        self.graph_layout.update(&mut gui, &ctx);
        self.dots_background.render(&mut gui, &ctx);
        self.render_connections(&mut gui, &mut ctx, execution_stats, interaction);

        let drag_port_info = self.node_ui.render_nodes(
            &mut gui,
            &mut ctx,
            &mut self.graph_layout,
            interaction,
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
                interaction,
            );
        }

        gui.set_scale(1.0);
        self.buttons(&mut gui, &mut ctx, interaction);
    }

    #[allow(clippy::too_many_arguments)]
    fn process_connections(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        background_response: &Response,
        pointer_pos: Pos2,
        drag_port_info: PortDragInfo,
        interaction: &mut GraphUiInteraction,
    ) {
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
            InteractionState::PanningGraph => {}
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
                        .chain(self.node_ui.const_bind_ui.broke_iter())
                        .cloned();

                    for connection in iter {
                        match connection {
                            ConnectionKey::Input {
                                input_node_id,
                                input_idx,
                            } => {
                                let node = ctx
                                    .view_graph
                                    .graph
                                    .nodes
                                    .by_key_mut(&input_node_id)
                                    .unwrap();
                                let input = &mut node.inputs[input_idx];
                                let before = input.binding.clone();
                                input.binding = Binding::None;
                                let after = input.binding.clone();
                                interaction.add_action(GraphUiAction::InputChanged {
                                    node_id: input_node_id,
                                    input_idx,
                                    before,
                                    after,
                                });
                            }
                            ConnectionKey::Event {
                                event_node_id,
                                event_idx,
                                trigger_node_id,
                            } => {
                                let node = ctx
                                    .view_graph
                                    .graph
                                    .nodes
                                    .by_key_mut(&event_node_id)
                                    .unwrap();
                                let event = &mut node.events[event_idx];
                                if event.subscribers.contains(&trigger_node_id) {
                                    event.subscribers.retain(|sub| *sub != trigger_node_id);
                                    interaction.add_action(GraphUiAction::EventConnectionChanged {
                                        event_node_id,
                                        event_idx,
                                        subscriber: trigger_node_id,
                                        change: EventSubscriberChange::Removed,
                                    });
                                }
                            }
                        }
                    }

                    for node_id in self.node_ui.node_ids_hit_breaker.iter() {
                        let action = ctx.view_graph.removal_action(node_id);
                        ctx.view_graph.remove_node(node_id);
                        interaction.add_action(action);
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
                        let input = &mut input_node.inputs[input_port.port_idx];
                        let before = input.binding.clone();
                        input.binding = Binding::Const(0.into());
                        let after = input.binding.clone();

                        interaction.add_action(GraphUiAction::InputChanged {
                            node_id: input_port.node_id,
                            input_idx: input_port.port_idx,
                            before,
                            after,
                        })
                    }
                    ConnectionDragUpdate::FinishedWith {
                        input_port,
                        output_port,
                    } => {
                        assert_eq!(input_port.kind, output_port.kind.opposite());

                        self.state = InteractionState::Idle;

                        match output_port.kind {
                            PortKind::Output => {
                                let result =
                                    apply_data_connection(ctx.view_graph, input_port, output_port);
                                match result {
                                    Ok(action) => interaction.add_action(action),
                                    Err(err) => interaction.add_error(err),
                                }
                            }
                            PortKind::Event => {
                                let result =
                                    apply_event_connection(ctx.view_graph, input_port, output_port);
                                match result {
                                    Ok(Some(action)) => interaction.add_action(action),
                                    Ok(None) => {}
                                    Err(err) => interaction.add_error(err),
                                }
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
        }
    }

    fn render_connections(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        execution_stats: Option<&ExecutionStats>,
        interaction: &mut GraphUiInteraction,
    ) {
        self.connections.render(
            gui,
            ctx,
            &self.graph_layout,
            execution_stats,
            interaction,
            if self.state == InteractionState::BreakingConnections {
                Some(&self.connection_breaker)
            } else {
                None
            },
        );

        if self.state == InteractionState::BreakingConnections {
            self.connection_breaker.show(gui);
        }
    }

    fn buttons(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext,
        interaction: &mut GraphUiInteraction,
    ) {
        let mut fit_all = false;
        let mut view_selected = false;
        let mut reset_view = false;

        let mono_font = gui.style.mono_font.clone();
        let small_padding = gui.style.small_padding;
        let padding = gui.style.padding;
        let rect = gui.rect;
        let egui_ctx = gui.ui().ctx().clone();
        let style_clone = gui.style.clone();

        {
            Area::new(Id::new("graph_ui_top_buttons"))
                .sizing_pass(false)
                .default_width(rect.width())
                .movable(false)
                .interactable(false)
                .fixed_pos(rect.min)
                .show(&egui_ctx, |ui| {
                    ui.set_clip_rect(rect);

                    ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                        ui.take_available_width();

                        Frame::NONE
                            .fill(Color32::from_black_alpha(128))
                            .inner_margin(padding)
                            .show(ui, |ui| {
                                ui.take_available_width();

                                ui.horizontal(|ui| {
                                    let mut make_button = |label| {
                                        let button_size =
                                            Vec2::splat(mono_font.size + small_padding * 2.0);
                                        ui.add_sized(
                                            button_size,
                                            egui::Button::new(
                                                RichText::new(label).font(mono_font.clone()),
                                            ),
                                        )
                                        .clicked()
                                    };
                                    fit_all = make_button("a");
                                    view_selected = make_button("s");
                                    reset_view = make_button("r");
                                });
                            });
                    });
                });
        }

        {
            Area::new(Id::new("graph_ui_bottom_buttons"))
                .fixed_pos(pos2(rect.left(), rect.bottom()))
                .pivot(Align2::LEFT_BOTTOM)
                .constrain_to(rect)
                .show(&egui_ctx, |ui| {
                    ui.set_clip_rect(rect);
                    ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                        Frame::NONE
                            .fill(Color32::from_gray(64))
                            .inner_margin(padding)
                            .show(ui, |ui| {
                                const BUTTON_WIDTH: f32 = 60.0;
                                const BUTTON_HEIGHT: f32 = 24.0;
                                const BUTTON_SPACING: f32 = 8.0;

                                let run_button_size = vec2(BUTTON_WIDTH, BUTTON_HEIGHT);

                                {
                                    ui.horizontal(|ui| {
                                        let mut gui = Gui::new(ui, style_clone.clone());

                                        let (run_rect, _) = gui
                                            .ui
                                            .allocate_exact_size(run_button_size, Sense::hover());

                                        let run_id =
                                            gui.ui().make_persistent_id("graph_run_button");
                                        let run_text_shape = {
                                            let font = gui.style.sub_font.clone();
                                            let color = gui.style.text_color;
                                            let galley = gui.ui().fonts_mut(|fonts| {
                                                fonts.layout_no_wrap("run".to_string(), font, color)
                                            });
                                            Shape::galley(run_rect.center(), galley, color)
                                        };
                                        let run_response = button::Button::new(run_id).show(
                                            &mut gui,
                                            run_rect,
                                            [run_text_shape],
                                        );

                                        interaction.run |= run_response.clicked();

                                        gui.ui.add_space(BUTTON_SPACING);

                                        let autorun_id =
                                            gui.ui.make_persistent_id("graph_autorun_toggle");
                                        let autorun_response =
                                            ToggleButton::new(autorun_id, "autorun")
                                                .checked(self.autorun_enabled)
                                                .show(&mut gui);

                                        if autorun_response.clicked() {
                                            if self.autorun_enabled {
                                                interaction.autorun = AutorunCommand::Stop;
                                            } else {
                                                interaction.autorun = AutorunCommand::Start;
                                            }
                                            self.autorun_enabled = !self.autorun_enabled;
                                        }
                                    });
                                }
                            });
                    });
                });
        }

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
        pointer_pos: Option<Pos2>,
        interaction: &mut GraphUiInteraction,
    ) {
        let prev_scale = ctx.view_graph.scale;
        let prev_pan = ctx.view_graph.pan;
        if let Some(pointer_pos) = pointer_pos {
            let (zoom_delta, pan) = {
                let (scroll_delta, mouse_wheel_delta) = collect_scroll_mouse_wheel_deltas(gui);

                (mouse_wheel_delta.abs() > f32::EPSILON).then_else(
                    ((mouse_wheel_delta * WHEEL_ZOOM_SPEED).exp(), Vec2::ZERO),
                    (
                        gui.ui().input(|input| {
                            input.modifiers.command.then_else(1.0, input.zoom_delta())
                        }),
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
        }

        match self.state {
            InteractionState::Idle => {
                if background_response.drag_started_by(PointerButton::Middle) {
                    self.state = InteractionState::PanningGraph;
                }
            }
            InteractionState::PanningGraph => {
                if background_response.drag_stopped_by(PointerButton::Middle) {
                    self.state = InteractionState::Idle;
                }
                if background_response.dragged_by(PointerButton::Middle) {
                    ctx.view_graph.pan += background_response.drag_delta();
                }
            }
            _ => {}
        }

        if !prev_scale.ui_equals(&ctx.view_graph.scale) || !prev_pan.ui_equals(&ctx.view_graph.pan)
        {
            interaction.add_action(GraphUiAction::ZoomPanChanged {
                before_pan: prev_pan,
                before_scale: prev_scale,
                after_pan: ctx.view_graph.pan,
                after_scale: ctx.view_graph.scale,
            });
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
fn apply_data_connection(
    view_graph: &mut model::ViewGraph,
    input_port: PortRef,
    output_port: PortRef,
) -> Result<GraphUiAction, Error> {
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
    let input = &mut input_node.inputs[input_port.port_idx];
    let before = input.binding.clone();
    let after = Binding::Bind(PortAddress {
        target_id: output_port.node_id,
        port_idx: output_port.port_idx,
    });
    input.binding = after.clone();

    Ok(GraphUiAction::InputChanged {
        node_id: input_port.node_id,
        input_idx: input_port.port_idx,
        before,
        after,
    })
}

/// Connects an event output port to a trigger input port in `view_graph`.
///
/// Returns the event node id, event index, and subscriber list before/after the change.
///
/// # Panics
/// Panics if the ports are not Event/Trigger or if the event index is out of range.
fn apply_event_connection(
    view_graph: &mut model::ViewGraph,
    input_port: PortRef,
    output_port: PortRef,
) -> Result<Option<GraphUiAction>, Error> {
    assert_eq!(input_port.kind, PortKind::Trigger);
    assert_eq!(output_port.kind, PortKind::Event);

    if input_port.node_id == output_port.node_id {
        return Err(Error::CycleDetected {
            input_node_id: input_port.node_id,
            output_node_id: output_port.node_id,
        });
    }

    let output_node = view_graph.graph.by_id_mut(&output_port.node_id).unwrap();
    assert!(
        output_port.port_idx < output_node.events.len(),
        "event index out of range for apply_event_connection"
    );
    let event = &mut output_node.events[output_port.port_idx];
    if event.subscribers.contains(&input_port.node_id) {
        return Ok(None);
    }

    event.subscribers.push(input_port.node_id);

    Ok(Some(GraphUiAction::EventConnectionChanged {
        event_node_id: output_port.node_id,
        event_idx: output_port.port_idx,
        subscriber: input_port.node_id,
        change: EventSubscriberChange::Added,
    }))
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
