use bumpalo::Bump;
use eframe::egui;
use egui::{
    Align2, Id, PointerButton, Pos2, Rect, Response, Sense, StrokeKind, UiBuilder, Vec2, pos2, vec2,
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

use crate::gui::connection_ui::{
    BrokeItem, ConnectionDragUpdate, ConnectionUi, advance_drag, disconnect_connection,
};
use crate::gui::connection_ui::{ConnectionKey, PortKind};
use crate::gui::graph_background::GraphBackgroundRenderer;
use crate::gui::graph_layout::{GraphLayout, PortRef};
use crate::gui::graph_ui_interaction::{GraphUiInteraction, RunCommand};
use crate::gui::interaction_state::Interaction;
use crate::gui::new_node_ui::{NewNodeSelection, NewNodeUi};
use crate::gui::node_details_ui::NodeDetailsUi;
use crate::gui::node_ui::{NodeUi, PortInteractCommand};
use crate::input::InputSnapshot;
use crate::{gui::Gui, gui::graph_ctx::GraphContext, model};
use common::BoolExt;

// ============================================================================
// Constants
// ============================================================================

const MIN_ZOOM: f32 = 0.2;
const MAX_ZOOM: f32 = 4.0;
const WHEEL_ZOOM_SPEED: f32 = 0.08;

#[derive(Debug)]
struct ButtonResult {
    response: Response,
    fit_all: bool,
    view_selected: bool,
    reset_view: bool,
}

// ============================================================================
// Types
// ============================================================================

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
    /// Centralized interaction state machine.
    interaction: Interaction,
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
        self.interaction.cancel();
    }

    // ------------------------------------------------------------------------
    // Main render entry point
    // ------------------------------------------------------------------------

    pub fn render(
        &mut self,
        gui: &mut Gui<'_>,
        app_data: &mut AppData,
        input: &InputSnapshot,
        arena: &Bump,
    ) {
        self.ui_interaction.clear();

        if input.cancel_requested() {
            self.cancel_interaction();
        }

        let rect = self.draw_background_frame(gui);

        gui.new_child(UiBuilder::new().id_salt("graph_ui").max_rect(rect), |gui| {
            gui.ui().set_clip_rect(rect);

            let mut ctx = GraphContext {
                func_lib: &app_data.func_lib,
                view_graph: &app_data.view_graph,
                execution_stats: app_data.execution_stats.as_ref(),
                autorun: app_data.autorun,
                argument_values_cache: &mut app_data.argument_values_cache,
            };

            let (background_response, pointer_pos) =
                self.setup_background_interaction(gui, input, rect);

            if background_response.clicked() {
                self.handle_background_click(&ctx);
            }

            // Phase 1: Graph content (layout, background, connections, nodes)
            gui.with_scale(ctx.view_graph.scale, |gui| {
                self.graph_layout.update(gui, &ctx, &self.interaction);
                self.dots_background.render(gui, &ctx);
                self.render_connections(gui, &ctx);

                let nodes_result = self.node_ui.render_nodes(
                    gui,
                    &ctx,
                    &mut self.graph_layout,
                    &mut self.ui_interaction,
                    &mut self.interaction,
                );

                // Surface the render's removal intents as actions. The
                // mutation itself happens in `NodeRemoved::apply` during
                // `handle_actions`.
                for node_id in &nodes_result.removed_nodes {
                    let action = ctx.view_graph.removal_action(node_id);
                    self.ui_interaction.add_action(action);
                }

                if let Some(pointer_pos) = pointer_pos {
                    self.process_connections(
                        gui,
                        input,
                        &ctx,
                        &background_response,
                        pointer_pos,
                        nodes_result.port_cmd,
                        &nodes_result.broken_nodes,
                    );
                }
            });

            // Phase 2: Overlays (buttons, details panel, new-node popup)
            let buttons = self.render_buttons(gui, ctx.autorun);
            if buttons.reset_view {
                self.emit_zoom_pan(ctx.view_graph, Vec2::ZERO, 1.0);
            }
            if buttons.view_selected
                && let Some((scale, pan)) = view_selected_node_target(gui, &ctx, &self.graph_layout)
            {
                self.emit_zoom_pan(ctx.view_graph, pan, scale);
            }
            if buttons.fit_all {
                let (scale, pan) = fit_all_nodes_target(gui, &ctx, &self.graph_layout);
                self.emit_zoom_pan(ctx.view_graph, pan, scale);
            }

            let mut overlay_hovered = buttons.response.hovered();
            overlay_hovered |= self
                .node_details_ui
                .show(gui, &mut ctx, &mut self.ui_interaction)
                .hovered();
            overlay_hovered |= self.handle_new_node_popup(
                gui,
                input,
                &ctx,
                pointer_pos,
                &background_response,
                arena,
            );

            // Phase 3: Zoom and pan (only when no overlay is hovered)
            if !overlay_hovered && (self.interaction.is_idle() || self.interaction.is_panning()) {
                self.update_zoom_and_pan(gui, input, &ctx, &background_response, pointer_pos);
            }
        });
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
        input: &InputSnapshot,
        rect: Rect,
    ) -> (Response, Option<Pos2>) {
        let graph_bg_id = gui.ui().make_persistent_id("graph_bg");

        let pointer_pos = input
            .pointer_pos
            .and_then(|pos| rect.contains(pos).then_else(Some(pos), None));

        let response = gui.ui().interact(
            rect,
            graph_bg_id,
            Sense::hover() | Sense::drag() | Sense::click(),
        );

        (response, pointer_pos)
    }

    fn handle_background_click(&mut self, ctx: &GraphContext<'_>) {
        self.cancel_interaction();

        if ctx.view_graph.selected_node_id.is_some() {
            let before = ctx.view_graph.selected_node_id;
            // Emit-action-only: selected_node_id is mutated by
            // `NodeSelected::apply` in handle_actions, not here.
            self.ui_interaction.add_action(GraphUiAction::NodeSelected {
                before,
                after: None,
            });
        }
    }

    // ------------------------------------------------------------------------
    // Connection processing
    // ------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn process_connections(
        &mut self,
        gui: &mut Gui<'_>,
        input: &InputSnapshot,
        ctx: &GraphContext<'_>,
        background_response: &Response,
        pointer_pos: Pos2,
        port_interact_cmd: PortInteractCommand,
        broken_nodes: &[NodeId],
    ) {
        let primary_down = input.primary_pressed || input.primary_down;

        match &mut self.interaction {
            // Node drag and view pan advance through egui response events
            // elsewhere in the frame; process_connections has nothing to
            // add for them.
            Interaction::Panning | Interaction::DraggingNode(_) => {}
            Interaction::Idle => {
                let pointer_on_background =
                    background_response.hovered() && !self.connections.any_hovered();
                handle_idle(
                    &mut self.interaction,
                    pointer_pos,
                    primary_down,
                    pointer_on_background,
                    port_interact_cmd,
                );
            }
            Interaction::BreakingConnections(breaker) => {
                Self::capture_overlay(gui);
                if primary_down {
                    breaker.add_point(pointer_pos);
                } else {
                    // Breaker released — collect results, then cancel.
                    self.apply_breaker_results(ctx, broken_nodes);
                    self.interaction.cancel();
                }
            }
            Interaction::DraggingConnection(drag) => {
                Self::capture_overlay(gui);
                let result = advance_drag(drag, pointer_pos, port_interact_cmd);
                self.handle_drag_result(ctx, pointer_pos, result);
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

    /// Collects all items hit by the breaker (connections, const bindings, nodes)
    /// and applies the corresponding removals in one pass.
    fn apply_breaker_results(&mut self, ctx: &GraphContext<'_>, broken_nodes: &[NodeId]) {
        let items: Vec<BrokeItem> = self
            .connections
            .broke_iter()
            .chain(self.node_ui.const_bind_ui.broke_iter())
            .map(|key| BrokeItem::Connection(*key))
            .chain(broken_nodes.iter().map(|id| BrokeItem::Node(*id)))
            .collect();

        for item in items {
            match item {
                BrokeItem::Connection(key) => {
                    disconnect_connection(key, ctx, &mut self.ui_interaction);
                }
                BrokeItem::Node(node_id) => {
                    let action = ctx.view_graph.removal_action(&node_id);
                    self.ui_interaction.add_action(action);
                }
            }
        }
    }

    fn handle_drag_result(
        &mut self,
        ctx: &GraphContext<'_>,
        pointer_pos: Pos2,
        result: ConnectionDragUpdate,
    ) {
        match result {
            ConnectionDragUpdate::InProgress => {}
            ConnectionDragUpdate::Finished => {
                self.interaction.cancel();
            }
            ConnectionDragUpdate::FinishedWithEmptyOutput { input_port } => {
                assert_eq!(input_port.kind, PortKind::Input);
                // NB: interaction stays in DraggingConnection so the ports
                // are available to `create_const_binding` if the user picks
                // ConstBind in the popup.
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
                self.interaction.cancel();
            }
        }
    }

    fn apply_connection(
        &mut self,
        ctx: &GraphContext<'_>,
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

    fn render_connections(&mut self, gui: &mut Gui<'_>, ctx: &GraphContext<'_>) {
        self.connections.render(
            gui,
            ctx,
            &self.graph_layout,
            &mut self.ui_interaction,
            self.interaction.breaker(),
        );

        match &mut self.interaction {
            Interaction::BreakingConnections(breaker) => {
                breaker.show(gui);
            }
            Interaction::DraggingConnection(drag) => {
                self.connections
                    .render_temp_connection(gui, &self.graph_layout, drag);
            }
            _ => {}
        }
    }

    fn render_buttons(&mut self, gui: &mut Gui<'_>, autorun: bool) -> ButtonResult {
        let mut autorun = autorun;
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
                            self.ui_interaction.set_run_cmd(RunCommand::RunOnce);
                        }

                        let response = Button::default()
                            .toggle(&mut autorun)
                            .text("autorun")
                            .show(gui);

                        if response.clicked() {
                            self.ui_interaction.set_run_cmd(if autorun {
                                RunCommand::StartAutorun
                            } else {
                                RunCommand::StopAutorun
                            });
                        }
                    });
                })
                .response
        })
        .inner;

        ButtonResult {
            response,
            fit_all,
            view_selected,
            reset_view,
        }
    }

    // ------------------------------------------------------------------------
    // New node popup
    // ------------------------------------------------------------------------

    fn handle_new_node_popup(
        &mut self,
        gui: &mut Gui<'_>,
        input: &InputSnapshot,
        ctx: &GraphContext<'_>,
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

        if let Some(selection) = self.new_node_ui.show(gui, input, ctx.func_lib, arena) {
            self.handle_new_node_selection(gui, ctx, selection);
        } else if was_open && !self.new_node_ui.is_open() {
            self.cancel_interaction();
        }

        self.new_node_ui.is_open()
    }

    fn handle_new_node_selection(
        &mut self,
        gui: &Gui<'_>,
        ctx: &GraphContext<'_>,
        selection: NewNodeSelection,
    ) {
        match selection {
            NewNodeSelection::Func(func) => {
                let screen_pos = self.new_node_ui.position();
                let origin = gui.rect.min;
                let graph_pos = (screen_pos - origin - ctx.view_graph.pan) / ctx.view_graph.scale;

                // Build the new node + view-node locally; apply() inserts them.
                let node: scenarium::graph::Node = func.into();
                let view_node = model::ViewNode {
                    id: node.id,
                    pos: graph_pos.to_pos2(),
                };

                self.ui_interaction
                    .add_action(GraphUiAction::NodeAdded { view_node, node });
            }
            NewNodeSelection::ConstBind => {
                self.create_const_binding(ctx);
                self.cancel_interaction();
            }
        }
    }

    fn create_const_binding(&mut self, ctx: &GraphContext<'_>) {
        let Some(connection_drag) = self.interaction.drag() else {
            return;
        };

        if connection_drag.start_port.port.kind != PortKind::Input {
            return;
        }

        let input_port = connection_drag.start_port.port;
        let input_node = ctx.view_graph.graph.by_id(&input_port.node_id).unwrap();
        let func_input =
            &ctx.func_lib.by_id(&input_node.func_id).unwrap().inputs[input_port.port_idx];
        let before = input_node.inputs[input_port.port_idx].binding.clone();
        let after: Binding = func_input
            .default_value
            .clone()
            .unwrap_or_else(|| StaticValue::from(&func_input.data_type))
            .into();

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
        input: &InputSnapshot,
        ctx: &GraphContext<'_>,
        background_response: &Response,
        pointer_pos: Option<Pos2>,
    ) {
        self.drive_pan_interaction_state(background_response);

        let mut new_scale = ctx.view_graph.scale;
        let mut new_pan = ctx.view_graph.pan;

        if let Some(pointer_pos) = pointer_pos {
            (new_scale, new_pan) = compute_scroll_zoom(gui, input, pointer_pos, new_scale, new_pan);
        }

        if matches!(self.interaction, Interaction::Panning)
            && background_response.dragged_by(PointerButton::Middle)
        {
            new_pan += background_response.drag_delta();
        }

        self.emit_zoom_pan(ctx.view_graph, new_pan, new_scale);
    }

    fn drive_pan_interaction_state(&mut self, background_response: &Response) {
        match &self.interaction {
            Interaction::Idle if background_response.drag_started_by(PointerButton::Middle) => {
                self.interaction.start_panning();
            }
            Interaction::Panning if background_response.drag_stopped_by(PointerButton::Middle) => {
                self.interaction.cancel();
            }
            _ => {}
        }
    }

    /// Emit `ZoomPanChanged` iff the target differs from the current view.
    /// `apply()` in `handle_actions` is the single site that writes
    /// `pan` / `scale` onto `ViewGraph`.
    fn emit_zoom_pan(
        &mut self,
        view_graph: &crate::model::ViewGraph,
        new_pan: Vec2,
        new_scale: f32,
    ) {
        if view_graph.scale.ui_equals(new_scale) && view_graph.pan.ui_equals(new_pan) {
            return;
        }
        self.ui_interaction
            .add_action(GraphUiAction::ZoomPanChanged {
                before_pan: view_graph.pan,
                before_scale: view_graph.scale,
                after_pan: new_pan,
                after_scale: new_scale,
            });
    }
}

// ============================================================================
// Free functions
// ============================================================================

/// Pure function: given the current (scale, pan) and this frame's scroll /
/// zoom input, return the target (scale, pan). No mutation — the orchestrator
/// emits a `ZoomPanChanged` action if the result differs from the current view.
fn compute_scroll_zoom(
    gui: &Gui<'_>,
    input: &InputSnapshot,
    pointer_pos: Pos2,
    current_scale: f32,
    current_pan: Vec2,
) -> (f32, Vec2) {
    let (scroll_delta, mouse_wheel_delta) = (input.scroll_delta, input.wheel_lines);

    let (zoom_delta, pan_delta) = (mouse_wheel_delta.abs() > f32::EPSILON).then_else(
        ((mouse_wheel_delta * WHEEL_ZOOM_SPEED).exp(), Vec2::ZERO),
        (input.zoom_delta_unless_cmd(), scroll_delta),
    );

    let mut new_scale = current_scale;
    let mut new_pan = current_pan;

    if (zoom_delta - 1.0).abs() > f32::EPSILON {
        let clamped_scale = (current_scale * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
        let origin = gui.rect.min;
        let graph_pos = (pointer_pos - origin - current_pan) / current_scale;
        new_scale = clamped_scale;
        new_pan = pointer_pos - origin - graph_pos * clamped_scale;
    }

    new_pan += pan_delta;
    (new_scale, new_pan)
}

/// Idle-state transitions driven by primary-button pressure plus a port
/// interaction command. Kept as a free function so it can borrow
/// `interaction` mutably without the enclosing `GraphUi::process_connections`
/// match needing to drop its discriminant borrow.
fn handle_idle(
    interaction: &mut Interaction,
    pointer_pos: Pos2,
    primary_down: bool,
    pointer_on_background: bool,
    port_interact_cmd: PortInteractCommand,
) {
    if !primary_down {
        return;
    }

    if let PortInteractCommand::DragStart(port_info) = port_interact_cmd {
        interaction.start_dragging(port_info);
    } else if pointer_on_background {
        interaction.start_breaking(pointer_pos);
    }
}

/// Builds the `InputChanged` action that would bind `input_port` to
/// `output_port`. No mutation — `apply` handles it.
fn apply_data_connection(
    view_graph: &model::ViewGraph,
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

    let input_node = view_graph.graph.by_id(&input_port.node_id).unwrap();
    let before = input_node.inputs[input_port.port_idx].binding.clone();
    let after = Binding::Bind(PortAddress {
        target_id: output_port.node_id,
        port_idx: output_port.port_idx,
    });

    Ok(GraphUiAction::InputChanged {
        node_id: input_port.node_id,
        input_idx: input_port.port_idx,
        before,
        after,
    })
}

/// Builds the `EventConnectionChanged` action that would subscribe
/// `input_port`'s node to `output_port`'s event. No mutation.
fn apply_event_connection(
    view_graph: &model::ViewGraph,
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

    let output_node = view_graph.graph.by_id(&output_port.node_id).unwrap();
    assert!(
        output_port.port_idx < output_node.events.len(),
        "event index out of range for apply_event_connection"
    );
    let event = &output_node.events[output_port.port_idx];

    if event.subscribers.contains(&input_port.node_id) {
        return Ok(None);
    }

    Ok(Some(GraphUiAction::EventConnectionChanged {
        event_node_id: output_port.node_id,
        event_idx: output_port.port_idx,
        subscriber: input_port.node_id,
        change: EventSubscriberChange::Added,
    }))
}

/// Computes target (scale, pan) to centre on the selected node, or `None`
/// if nothing is selected / the layout isn't known.
fn view_selected_node_target(
    gui: &Gui<'_>,
    ctx: &GraphContext<'_>,
    graph_layout: &GraphLayout,
) -> Option<(f32, Vec2)> {
    let selected_id = ctx.view_graph.selected_node_id?;
    let node_view = ctx.view_graph.view_nodes.by_key(&selected_id)?;

    let scale = ctx.view_graph.scale;
    let rect = graph_layout.node_layout(&node_view.id).body_rect;
    let size = rect.size() / scale;
    let center = egui::pos2(
        node_view.pos.x + size.x * 0.5,
        node_view.pos.y + size.y * 0.5,
    );

    let target_scale = 1.0;
    let target_pan = gui.rect.center() - gui.rect.min - center.to_vec2();
    Some((target_scale, target_pan))
}

/// Computes target (scale, pan) that fits all nodes on screen. Returns
/// `(1.0, Vec2::ZERO)` when the graph is empty so the caller can still
/// emit a normalised ZoomPanChanged when the user hits "fit all" on an
/// empty graph.
fn fit_all_nodes_target(
    gui: &Gui<'_>,
    ctx: &GraphContext<'_>,
    graph_layout: &GraphLayout,
) -> (f32, Vec2) {
    if ctx.view_graph.view_nodes.is_empty() {
        return (1.0, Vec2::ZERO);
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

    let target_scale = zoom_x.min(zoom_y).clamp(MIN_ZOOM, MAX_ZOOM);
    let bounds_center = bounds.center().to_vec2();
    let target_pan = gui.rect.center() - gui.rect.min - bounds_center * target_scale;
    (target_scale, target_pan)
}
