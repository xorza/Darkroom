use bumpalo::Bump;
use eframe::egui;
use egui::{
    Align2, Id, Key, PointerButton, Pos2, Rect, Response, Sense, StrokeKind, UiBuilder, Vec2, pos2,
    vec2,
};
use scenarium::data::StaticValue;

use crate::app_data::AppData;
use crate::common::frame::Frame;
use crate::common::positioned_ui::PositionedUi;
use crate::model::EventSubscriberChange;
use crate::model::graph_ui_action::GraphUiAction;
use scenarium::graph::NodeId;
use scenarium::prelude::{Binding, PortAddress};

use crate::common::UiEquals;

use crate::common::button::Button;

use crate::gui::connection_ui::{BrokeItem, ConnectionDragUpdate, ConnectionUi};
use crate::gui::connection_ui::{ConnectionKey, PortKind};
use crate::gui::graph_background::GraphBackgroundRenderer;
use crate::gui::graph_layout::{GraphLayout, PortRef};
use crate::gui::graph_ui_interaction::{GraphUiInteraction, RunCommand};
use crate::gui::interaction_state::{GraphInteractionState, InteractionMode};
use crate::gui::new_node_ui::{NewNodeSelection, NewNodeUi};
use crate::gui::node_details_ui::NodeDetailsUi;
use crate::gui::node_ui::{NodeUi, PortInteractCommand};
use crate::{gui::Gui, gui::graph_ctx::GraphContext, model};
use common::BoolExt;

// ============================================================================
// Constants
// ============================================================================

const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;
const WHEEL_ZOOM_SPEED: f32 = 0.08;

// ============================================================================
// Types
// ============================================================================

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

// ============================================================================
// GraphUi
// ============================================================================

#[derive(Debug, Default)]
pub struct GraphUi {
    /// Centralized interaction state management.
    interaction: GraphInteractionState,
    connections: ConnectionUi,
    graph_layout: GraphLayout,
    node_ui: NodeUi,
    dots_background: GraphBackgroundRenderer,
    new_node_ui: NewNodeUi,
    node_details_ui: NodeDetailsUi,
    ui_interaction: GraphUiInteraction,
}

impl GraphUi {
    pub fn ui_interaction(&mut self) -> &mut GraphUiInteraction {
        &mut self.ui_interaction
    }

    fn cancel_interaction(&mut self) {
        self.interaction.reset_to_idle();
        self.connections.stop_drag();
    }

    // ------------------------------------------------------------------------
    // Main render entry point
    // ------------------------------------------------------------------------

    pub fn render(&mut self, gui: &mut Gui<'_>, app_data: &mut AppData, arena: &Bump) {
        self.ui_interaction.clear();

        if self.should_cancel_interaction(gui) {
            self.cancel_interaction();
        }

        let rect = self.draw_background_frame(gui);

        gui.new_child(UiBuilder::new().id_salt("graph_ui").max_rect(rect), |gui| {
            gui.ui().set_clip_rect(rect);

            let mut ctx = GraphContext {
                func_lib: &app_data.func_lib,
                view_graph: &mut app_data.view_graph,
                execution_stats: app_data.execution_stats.as_ref(),
                autorun: app_data.autorun,
                argument_values_cache: &mut app_data.argument_values_cache,
            };

            let (background_response, pointer_pos) = self.setup_background_interaction(gui, rect);

            if background_response.clicked() {
                self.handle_background_click(&mut ctx);
            }

            // Phase 1: Graph content (layout, background, connections, nodes)
            gui.set_scale(ctx.view_graph.scale);

            self.graph_layout.update(gui, &ctx);
            self.dots_background.render(gui, &ctx);
            self.render_connections(gui, &mut ctx);

            let port_interact_cmd = self.node_ui.render_nodes(
                gui,
                &mut ctx,
                &mut self.graph_layout,
                &mut self.ui_interaction,
                self.interaction.breaker(),
            );

            if let Some(pointer_pos) = pointer_pos {
                self.process_connections(
                    gui,
                    &mut ctx,
                    &background_response,
                    pointer_pos,
                    port_interact_cmd,
                );
            }

            // Phase 2: Overlays (buttons, details panel, new-node popup)
            gui.set_scale(1.0);

            let mut overlay_hovered = self.render_buttons(gui, &mut ctx).hovered();
            overlay_hovered |= self
                .node_details_ui
                .show(gui, &mut ctx, &mut self.ui_interaction)
                .hovered();
            overlay_hovered |=
                self.handle_new_node_popup(gui, &mut ctx, pointer_pos, &background_response, arena);

            // Phase 3: Zoom and pan (only when no overlay is hovered)
            if !overlay_hovered && (self.interaction.is_idle() || self.interaction.is_panning()) {
                self.update_zoom_and_pan(gui, &mut ctx, &background_response, pointer_pos);
            }
        });
    }

    // ------------------------------------------------------------------------
    // Input helpers
    // ------------------------------------------------------------------------

    fn should_cancel_interaction(&self, gui: &mut Gui<'_>) -> bool {
        gui.ui()
            .input(|input| input.key_pressed(Key::Escape) || input.pointer.secondary_pressed())
    }

    fn get_primary_button_state(gui: &mut Gui<'_>) -> Option<PointerButtonState> {
        gui.ui().input(|input| {
            if input.pointer.primary_pressed() {
                Some(PointerButtonState::Pressed)
            } else if input.pointer.primary_released() {
                Some(PointerButtonState::Released)
            } else if input.pointer.primary_down() {
                Some(PointerButtonState::Down)
            } else {
                None
            }
        })
    }

    // ------------------------------------------------------------------------
    // Background setup
    // ------------------------------------------------------------------------

    fn draw_background_frame(&self, gui: &mut Gui<'_>) -> Rect {
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

        rect.shrink(gui.style.corner_radius * 0.5)
    }

    fn setup_background_interaction(
        &self,
        gui: &mut Gui<'_>,
        rect: Rect,
    ) -> (Response, Option<Pos2>) {
        let graph_bg_id = gui.ui().make_persistent_id("graph_bg");

        let pointer_pos = gui
            .ui()
            .input(|input| input.pointer.hover_pos())
            .and_then(|pos| rect.contains(pos).then_else(Some(pos), None));

        let response = gui.ui().interact(
            rect,
            graph_bg_id,
            Sense::hover() | Sense::drag() | Sense::click(),
        );

        (response, pointer_pos)
    }

    fn handle_background_click(&mut self, ctx: &mut GraphContext<'_>) {
        self.cancel_interaction();

        if ctx.view_graph.selected_node_id.is_some() {
            let before = ctx.view_graph.selected_node_id;
            ctx.view_graph.selected_node_id = None;
            self.ui_interaction.add_action(GraphUiAction::NodeSelected {
                before,
                after: None,
            });
        }
    }

    // ------------------------------------------------------------------------
    // Connection processing
    // ------------------------------------------------------------------------

    fn process_connections(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        background_response: &Response,
        pointer_pos: Pos2,
        port_interact_cmd: PortInteractCommand,
    ) {
        let primary_down = matches!(
            Self::get_primary_button_state(gui),
            Some(PointerButtonState::Pressed | PointerButtonState::Down)
        );

        match self.interaction.mode() {
            InteractionMode::PanningGraph => {}
            InteractionMode::Idle => {
                let pointer_on_background =
                    background_response.hovered() && !self.connections.any_hovered();
                self.handle_idle_state(
                    pointer_pos,
                    primary_down,
                    pointer_on_background,
                    port_interact_cmd,
                );
            }
            InteractionMode::BreakingConnections => {
                Self::capture_overlay(gui);
                self.handle_breaking_connections(ctx, pointer_pos, primary_down);
            }
            InteractionMode::DraggingNewConnection => {
                Self::capture_overlay(gui);
                self.handle_dragging_connection(ctx, pointer_pos, port_interact_cmd);
            }
        }
    }

    /// Captures all input over the graph area to prevent background interaction
    /// while breaking connections or dragging a new connection.
    fn capture_overlay(gui: &mut Gui<'_>) {
        let id = gui.ui().make_persistent_id("temp overlay background");
        let rect = gui.rect;
        gui.ui().interact(rect, id, Sense::all());
    }

    fn handle_idle_state(
        &mut self,
        pointer_pos: Pos2,
        primary_down: bool,
        pointer_on_background: bool,
        port_interact_cmd: PortInteractCommand,
    ) {
        if !primary_down {
            return;
        }

        if let PortInteractCommand::DragStart(_) = port_interact_cmd {
            self.interaction.start_dragging_connection();
            self.connections.update_drag(pointer_pos, port_interact_cmd);
        } else if pointer_on_background {
            self.interaction.start_breaking(pointer_pos);
        }
    }

    fn handle_breaking_connections(
        &mut self,
        ctx: &mut GraphContext<'_>,
        pointer_pos: Pos2,
        primary_down: bool,
    ) {
        if primary_down {
            self.interaction.add_breaker_point(pointer_pos);
            return;
        }

        self.apply_breaker_results(ctx);
        self.cancel_interaction();
    }

    /// Collects all items hit by the breaker (connections, const bindings, nodes)
    /// and applies the corresponding removals in one pass.
    fn apply_breaker_results(&mut self, ctx: &mut GraphContext<'_>) {
        let items: Vec<BrokeItem> = self
            .connections
            .broke_iter()
            .chain(self.node_ui.const_bind_ui.broke_iter())
            .map(|key| BrokeItem::Connection(*key))
            .chain(
                self.node_ui
                    .broke_node_iter()
                    .map(|id| BrokeItem::Node(*id)),
            )
            .collect();

        for item in items {
            match item {
                BrokeItem::Connection(ConnectionKey::Input {
                    input_node_id,
                    input_idx,
                }) => {
                    let node = ctx
                        .view_graph
                        .graph
                        .nodes
                        .by_key_mut(&input_node_id)
                        .unwrap();
                    let input = &mut node.inputs[input_idx];
                    let before = input.binding.clone();
                    input.binding = Binding::None;

                    self.ui_interaction.add_action(GraphUiAction::InputChanged {
                        node_id: input_node_id,
                        input_idx,
                        before,
                        after: Binding::None,
                    });
                }
                BrokeItem::Connection(ConnectionKey::Event {
                    event_node_id,
                    event_idx,
                    trigger_node_id,
                }) => {
                    let node = ctx
                        .view_graph
                        .graph
                        .nodes
                        .by_key_mut(&event_node_id)
                        .unwrap();
                    let event = &mut node.events[event_idx];

                    if event.subscribers.contains(&trigger_node_id) {
                        event.subscribers.retain(|sub| *sub != trigger_node_id);
                        self.ui_interaction
                            .add_action(GraphUiAction::EventConnectionChanged {
                                event_node_id,
                                event_idx,
                                subscriber: trigger_node_id,
                                change: EventSubscriberChange::Removed,
                            });
                    }
                }
                BrokeItem::Node(node_id) => {
                    let action = ctx.view_graph.removal_action(&node_id);
                    ctx.view_graph.remove_node(&node_id);
                    self.ui_interaction.add_action(action);
                }
            }
        }
    }

    fn handle_dragging_connection(
        &mut self,
        ctx: &mut GraphContext<'_>,
        pointer_pos: Pos2,
        port_interact_cmd: PortInteractCommand,
    ) {
        match self.connections.update_drag(pointer_pos, port_interact_cmd) {
            ConnectionDragUpdate::InProgress => {}
            ConnectionDragUpdate::Finished => {
                self.cancel_interaction();
            }
            ConnectionDragUpdate::FinishedWithEmptyOutput { input_port } => {
                assert_eq!(input_port.kind, PortKind::Input);
                self.new_node_ui.open_from_connection(pointer_pos);
            }
            ConnectionDragUpdate::FinishedWithEmptyInput { output_port } => {
                assert_eq!(output_port.kind, PortKind::Output);
                self.new_node_ui.open(pointer_pos);
            }
            ConnectionDragUpdate::FinishedWith {
                input_port,
                output_port,
            } => {
                self.apply_connection(ctx, input_port, output_port);
                self.cancel_interaction();
            }
        }
    }

    fn apply_connection(
        &mut self,
        ctx: &mut GraphContext<'_>,
        input_port: PortRef,
        output_port: PortRef,
    ) {
        assert_eq!(input_port.kind, output_port.kind.opposite());

        match output_port.kind {
            PortKind::Output => {
                match apply_data_connection(ctx.view_graph, input_port, output_port) {
                    Ok(action) => self.ui_interaction.add_action(action),
                    Err(err) => self.ui_interaction.add_error(err),
                }
            }
            PortKind::Event => {
                match apply_event_connection(ctx.view_graph, input_port, output_port) {
                    Ok(Some(action)) => self.ui_interaction.add_action(action),
                    Ok(None) => {}
                    Err(err) => self.ui_interaction.add_error(err),
                }
            }
            _ => unreachable!(),
        }
    }

    // ------------------------------------------------------------------------
    // Rendering
    // ------------------------------------------------------------------------

    fn render_connections(&mut self, gui: &mut Gui<'_>, ctx: &mut GraphContext<'_>) {
        self.connections.render(
            gui,
            ctx,
            &self.graph_layout,
            &mut self.ui_interaction,
            self.interaction.breaker(),
        );

        if self.interaction.is_breaking_connections() {
            self.interaction.breaker_mut().show(gui);
        }
    }

    fn render_buttons(&mut self, gui: &mut Gui<'_>, ctx: &mut GraphContext) -> Response {
        let mut autorun = ctx.autorun;
        let rect = gui.rect;
        let mut fit_all = false;
        let mut view_selected = false;
        let mut reset_view = false;

        // Top buttons (view controls)
        let mut response = PositionedUi::new(Id::new("graph_ui_top_buttons"), rect.left_top())
            .pivot(Align2::LEFT_TOP)
            .interactable(false)
            .show(gui, |gui| {
                gui.ui().take_available_width();
                let padding = gui.style.padding;

                Frame::none()
                    .sense(Sense::all())
                    .inner_margin(padding)
                    .show(gui, |gui| {
                        gui.horizontal(|gui| {
                            let btn_size = vec2(20.0, 20.0);
                            let mono_font = gui.style.mono_font.clone();

                            let response = Button::default()
                                .text("a")
                                .font(mono_font.clone())
                                .size(btn_size)
                                .show(gui);
                            fit_all = response.clicked();

                            let response = Button::default()
                                .text("s")
                                .font(mono_font.clone())
                                .size(btn_size)
                                .show(gui);
                            view_selected = response.clicked();

                            let response = Button::default()
                                .text("r")
                                .font(mono_font)
                                .size(btn_size)
                                .show(gui);
                            reset_view = response.clicked();
                        });
                    })
                    .response
            })
            .inner;

        // Bottom buttons (execution controls)
        response |= PositionedUi::new(
            Id::new("graph_ui_bottom_buttons"),
            pos2(rect.left(), rect.bottom()),
        )
        .pivot(Align2::LEFT_BOTTOM)
        .interactable(false)
        .show(gui, |gui| {
            let padding = gui.style.padding;

            Frame::none()
                .sense(Sense::all())
                .inner_margin(padding)
                .show(gui, |gui| {
                    gui.horizontal(|gui| {
                        let response = Button::default().text("run").show(gui);
                        if response.clicked() {
                            self.ui_interaction.run_cmd = RunCommand::RunOnce;
                        }

                        let response = Button::default()
                            .toggle(&mut autorun)
                            .text("autorun")
                            .show(gui);

                        if response.clicked() {
                            self.ui_interaction.run_cmd = if autorun {
                                RunCommand::StartAutorun
                            } else {
                                RunCommand::StopAutorun
                            };
                        }
                    });
                })
                .response
        })
        .inner;

        // Apply view actions
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

        response
    }

    // ------------------------------------------------------------------------
    // New node popup
    // ------------------------------------------------------------------------

    fn handle_new_node_popup(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        pointer_pos: Option<Pos2>,
        background_response: &Response,
        arena: &Bump,
    ) -> bool {
        if background_response.double_clicked_by(PointerButton::Primary)
            && let Some(pos) = pointer_pos
        {
            self.new_node_ui.open(pos);
        }

        let was_open = self.new_node_ui.is_open();

        if let Some(selection) = self.new_node_ui.show(gui, ctx.func_lib, arena) {
            self.handle_new_node_selection(gui, ctx, selection);
        } else if was_open && !self.new_node_ui.is_open() {
            self.cancel_interaction();
        }

        self.new_node_ui.is_open()
    }

    fn handle_new_node_selection(
        &mut self,
        gui: &Gui<'_>,
        ctx: &mut GraphContext<'_>,
        selection: NewNodeSelection,
    ) {
        match selection {
            NewNodeSelection::Func(func) => {
                let screen_pos = self.new_node_ui.position();
                let origin = gui.rect.min;
                let graph_pos = (screen_pos - origin - ctx.view_graph.pan) / ctx.view_graph.scale;

                let (node, view_node) = ctx.view_graph.add_node_from_func(func);
                view_node.pos = graph_pos.to_pos2();

                self.ui_interaction.add_action(GraphUiAction::NodeAdded {
                    view_node: view_node.clone(),
                    node: node.clone(),
                });
            }
            NewNodeSelection::ConstBind => {
                self.create_const_binding(ctx);
                self.cancel_interaction();
            }
        }
    }

    fn create_const_binding(&mut self, ctx: &mut GraphContext<'_>) {
        let Some(connection_drag) = self.connections.temp_connection.as_ref() else {
            return;
        };

        if connection_drag.start_port.port.kind != PortKind::Input {
            return;
        }

        let input_port = connection_drag.start_port.port;
        let input_node = ctx.view_graph.graph.by_id_mut(&input_port.node_id).unwrap();
        let func_input =
            &ctx.func_lib.by_id(&input_node.func_id).unwrap().inputs[input_port.port_idx];
        let input = &mut input_node.inputs[input_port.port_idx];

        let before = input.binding.clone();
        input.binding = func_input
            .default_value
            .clone()
            .unwrap_or_else(|| StaticValue::from(&func_input.data_type))
            .into();
        let after = input.binding.clone();

        self.ui_interaction.add_action(GraphUiAction::InputChanged {
            node_id: input_port.node_id,
            input_idx: input_port.port_idx,
            before,
            after,
        });
    }

    // ------------------------------------------------------------------------
    // Zoom and pan
    // ------------------------------------------------------------------------

    fn update_zoom_and_pan(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        background_response: &Response,
        pointer_pos: Option<Pos2>,
    ) {
        let prev_scale = ctx.view_graph.scale;
        let prev_pan = ctx.view_graph.pan;

        if let Some(pointer_pos) = pointer_pos {
            self.apply_scroll_zoom(gui, ctx, pointer_pos);
        }

        self.handle_pan_state(ctx, background_response);

        if !prev_scale.ui_equals(ctx.view_graph.scale) || !prev_pan.ui_equals(ctx.view_graph.pan) {
            self.ui_interaction
                .add_action(GraphUiAction::ZoomPanChanged {
                    before_pan: prev_pan,
                    before_scale: prev_scale,
                    after_pan: ctx.view_graph.pan,
                    after_scale: ctx.view_graph.scale,
                });
        }
    }

    fn apply_scroll_zoom(&self, gui: &mut Gui<'_>, ctx: &mut GraphContext<'_>, pointer_pos: Pos2) {
        let (scroll_delta, mouse_wheel_delta) = collect_scroll_mouse_wheel_deltas(gui);

        let (zoom_delta, pan) = (mouse_wheel_delta.abs() > f32::EPSILON).then_else(
            ((mouse_wheel_delta * WHEEL_ZOOM_SPEED).exp(), Vec2::ZERO),
            (
                gui.ui()
                    .input(|input| input.modifiers.command.then_else(1.0, input.zoom_delta())),
                scroll_delta,
            ),
        );

        if (zoom_delta - 1.0).abs() > f32::EPSILON {
            let clamped_scale = (ctx.view_graph.scale * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
            let origin = gui.rect.min;
            let graph_pos = (pointer_pos - origin - ctx.view_graph.pan) / ctx.view_graph.scale;
            ctx.view_graph.scale = clamped_scale;
            ctx.view_graph.pan = pointer_pos - origin - graph_pos * ctx.view_graph.scale;
        }

        ctx.view_graph.pan += pan;
    }

    fn handle_pan_state(&mut self, ctx: &mut GraphContext<'_>, background_response: &Response) {
        match self.interaction.mode() {
            InteractionMode::Idle => {
                if background_response.drag_started_by(PointerButton::Middle) {
                    self.interaction.start_panning();
                }
            }
            InteractionMode::PanningGraph => {
                if background_response.drag_stopped_by(PointerButton::Middle) {
                    self.cancel_interaction();
                }
                if background_response.dragged_by(PointerButton::Middle) {
                    ctx.view_graph.pan += background_response.drag_delta();
                }
            }
            _ => {}
        }
    }
}

// ============================================================================
// Free functions
// ============================================================================

/// Returns smooth scroll delta plus an accumulated mouse-wheel line/page magnitude.
fn collect_scroll_mouse_wheel_deltas(gui: &mut Gui<'_>) -> (Vec2, f32) {
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
}

/// Connects an output port to an input port in `view_graph`.
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
    let after = Binding::Bind(PortAddress {
        target_id: output_port.node_id,
        port_idx: output_port.port_idx,
    });
    let before = std::mem::replace(&mut input.binding, after.clone());

    Ok(GraphUiAction::InputChanged {
        node_id: input_port.node_id,
        input_idx: input_port.port_idx,
        before,
        after,
    })
}

/// Connects an event output port to a trigger input port in `view_graph`.
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
