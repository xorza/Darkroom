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

mod connections;
mod overlays;
mod pan_zoom;

use pan_zoom::{fit_all_nodes_target, view_selected_node_target};

pub(crate) const MIN_ZOOM: f32 = 0.2;
pub(crate) const MAX_ZOOM: f32 = 4.0;
pub(crate) const WHEEL_ZOOM_SPEED: f32 = 0.08;

#[derive(Debug)]
pub(super) struct ButtonResult {
    pub(super) response: Response,
    pub(super) fit_all: bool,
    pub(super) view_selected: bool,
    pub(super) reset_view: bool,
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
    /// One of the ports the action refers to no longer exists. Happens if
    /// a node is deleted (e.g. via undo/redo) while the user is holding
    /// an in-flight drag whose endpoints referenced that node.
    StaleNode { node_id: NodeId },
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
            Error::StaleNode { node_id } => write!(
                f,
                "node {node_id} referenced by in-flight drag no longer exists"
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

    /// Cancels the current interaction if it references nodes that no
    /// longer exist in `view_graph`. See `Error::StaleNode`.
    fn drop_stale_interaction(&mut self, view_graph: &model::ViewGraph) {
        let stale = match &self.interaction {
            Interaction::Idle | Interaction::Panning | Interaction::BreakingConnections(_) => false,
            Interaction::DraggingNode(drag) => view_graph.graph.by_id(&drag.node_id).is_none(),
            Interaction::DraggingConnection(drag) => {
                let start_missing = view_graph
                    .graph
                    .by_id(&drag.start_port.port.node_id)
                    .is_none();
                let end_missing = drag
                    .end_port
                    .is_some_and(|p| view_graph.graph.by_id(&p.port.node_id).is_none());
                start_missing || end_missing
            }
        };
        if stale {
            self.interaction.cancel();
        }
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

        // Drop any interaction state that references nodes that have
        // since been removed (e.g. by an undo/redo that ran between
        // frames). Keeping stale IDs in `Interaction` would propagate to
        // downstream `.unwrap()` sites in drag/commit code paths.
        self.drop_stale_interaction(&app_data.state.view_graph);

        let rect = self.draw_background_frame(gui);

        gui.new_child(UiBuilder::new().id_salt("graph_ui").max_rect(rect), |gui| {
            gui.ui().set_clip_rect(rect);

            let mut ctx = GraphContext {
                func_lib: &app_data.state.func_lib,
                view_graph: &app_data.state.view_graph,
                execution_stats: app_data.state.execution_stats.as_ref(),
                autorun: app_data.state.autorun,
                argument_values_cache: &mut app_data.state.argument_values_cache,
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

                // Overlay that swallows background click-through for
                // active gestures. Registered HERE — before ports — so
                // later-registered port widgets keep higher egui
                // z-order and their click/drag responses still fire
                // through. See `Self::maybe_capture_overlay`.
                Self::maybe_capture_overlay(gui, &self.interaction);

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

    // NB: connection-related methods (handle_background_click,
    // process_connections, render_connections, apply_breaker_results,
    // handle_drag_result, apply_connection, capture_overlay) live in
    // the `connections` submodule.

    // NB: render_buttons / handle_new_node_popup / handle_new_node_selection /
    // create_const_binding live in the `overlays` submodule; pan/zoom methods
    // in the `pan_zoom` submodule; connection methods in `connections`.
}

// ============================================================================
// Free functions
// ============================================================================

// ============================================================================
// Tests
// ============================================================================
//
// These exercise the pure helpers that sit at the render → action boundary:
// they take an immutable `ViewGraph` and `PortRef`s and return actions, so
// they're unit-testable without any egui runtime. Historically these paths
// carried the cycle-detection and subscribe/unsubscribe logic, which is
// where most connection-related bugs live.

#[cfg(test)]
mod tests {
    use super::connections::{
        build_data_connection_action, build_event_connection_action, handle_idle,
    };
    use super::*;
    use crate::gui::graph_layout::PortInfo;
    use scenarium::function::FuncId;
    use scenarium::graph::{Event, Input, Node, NodeBehavior};

    fn make_node_with(input_count: usize, event_count: usize) -> Node {
        Node {
            id: NodeId::unique(),
            func_id: FuncId::unique(),
            name: String::new(),
            behavior: NodeBehavior::AsFunction,
            inputs: (0..input_count).map(|_| Input::default()).collect(),
            events: (0..event_count)
                .map(|_| Event {
                    name: String::new(),
                    subscribers: Vec::new(),
                })
                .collect(),
        }
    }

    fn view_graph_with_nodes(nodes: Vec<Node>) -> model::ViewGraph {
        let mut vg = model::ViewGraph::default();
        for node in nodes {
            let view_node = model::ViewNode {
                id: node.id,
                pos: Pos2::ZERO,
            };
            vg.view_nodes.add(view_node);
            vg.graph.add(node);
        }
        vg
    }

    fn port(node_id: NodeId, kind: PortKind, port_idx: usize) -> PortRef {
        PortRef {
            node_id,
            kind,
            port_idx,
        }
    }

    // --- build_data_connection_action ------------------------------------------

    #[test]
    fn build_data_connection_action_rejects_self_loop() {
        let a = make_node_with(1, 0);
        let a_id = a.id;
        let vg = view_graph_with_nodes(vec![a]);

        let err = build_data_connection_action(
            &vg,
            port(a_id, PortKind::Input, 0),
            port(a_id, PortKind::Output, 0),
        )
        .unwrap_err();
        assert!(matches!(err, Error::CycleDetected { .. }));
    }

    #[test]
    fn build_data_connection_action_detects_transitive_cycle() {
        // a.input0 <- b.output0 already wired; trying to wire
        // b.input0 <- a.output0 would create a cycle.
        let mut a = make_node_with(1, 0);
        let b = make_node_with(1, 0);
        let a_id = a.id;
        let b_id = b.id;

        a.inputs[0].binding = Binding::Bind(PortAddress {
            target_id: b_id,
            port_idx: 0,
        });
        let vg = view_graph_with_nodes(vec![a, b]);

        let err = build_data_connection_action(
            &vg,
            port(b_id, PortKind::Input, 0),
            port(a_id, PortKind::Output, 0),
        )
        .unwrap_err();
        assert!(matches!(err, Error::CycleDetected { .. }));
    }

    #[test]
    fn build_data_connection_action_produces_input_changed() {
        let a = make_node_with(1, 0);
        let b = make_node_with(1, 0);
        let a_id = a.id;
        let b_id = b.id;
        let vg = view_graph_with_nodes(vec![a, b]);

        let action = build_data_connection_action(
            &vg,
            port(a_id, PortKind::Input, 0),
            port(b_id, PortKind::Output, 0),
        )
        .unwrap();

        match action {
            GraphUiAction::InputChanged {
                node_id,
                input_idx,
                before,
                after,
            } => {
                assert_eq!(node_id, a_id);
                assert_eq!(input_idx, 0);
                assert!(matches!(before, Binding::None));
                match after {
                    Binding::Bind(addr) => {
                        assert_eq!(addr.target_id, b_id);
                        assert_eq!(addr.port_idx, 0);
                    }
                    _ => panic!("expected Binding::Bind"),
                }
            }
            other => panic!("expected InputChanged, got {other:?}"),
        }
    }

    // --- build_event_connection_action -----------------------------------------

    #[test]
    fn build_event_connection_action_rejects_self_loop() {
        let a = make_node_with(0, 1);
        let a_id = a.id;
        let vg = view_graph_with_nodes(vec![a]);

        let err = build_event_connection_action(
            &vg,
            port(a_id, PortKind::Trigger, 0),
            port(a_id, PortKind::Event, 0),
        )
        .unwrap_err();
        assert!(matches!(err, Error::CycleDetected { .. }));
    }

    #[test]
    fn build_event_connection_action_no_op_when_already_subscribed() {
        let mut a = make_node_with(0, 1);
        let b = make_node_with(0, 0);
        let a_id = a.id;
        let b_id = b.id;
        a.events[0].subscribers.push(b_id);
        let vg = view_graph_with_nodes(vec![a, b]);

        let result = build_event_connection_action(
            &vg,
            port(b_id, PortKind::Trigger, 0),
            port(a_id, PortKind::Event, 0),
        )
        .unwrap();
        assert!(result.is_none(), "re-subscribing must be a no-op");
    }

    #[test]
    fn build_data_connection_action_stale_input_node_yields_stale_error() {
        // Drag started from node A, but A was removed between frames —
        // only B is left. We should get Error::StaleNode, not a panic.
        let b = make_node_with(0, 0);
        let b_id = b.id;
        let vg = view_graph_with_nodes(vec![b]);
        let missing = NodeId::unique();

        let err = build_data_connection_action(
            &vg,
            port(missing, PortKind::Input, 0),
            port(b_id, PortKind::Output, 0),
        )
        .unwrap_err();
        assert!(matches!(err, Error::StaleNode { node_id } if node_id == missing));
    }

    #[test]
    fn build_event_connection_action_stale_output_node_yields_stale_error() {
        let a = make_node_with(0, 0);
        let a_id = a.id;
        let vg = view_graph_with_nodes(vec![a]);
        let missing = NodeId::unique();

        let err = build_event_connection_action(
            &vg,
            port(a_id, PortKind::Trigger, 0),
            port(missing, PortKind::Event, 0),
        )
        .unwrap_err();
        assert!(matches!(err, Error::StaleNode { node_id } if node_id == missing));
    }

    #[test]
    fn build_event_connection_action_subscribes() {
        let a = make_node_with(0, 1);
        let b = make_node_with(0, 0);
        let a_id = a.id;
        let b_id = b.id;
        let vg = view_graph_with_nodes(vec![a, b]);

        let action = build_event_connection_action(
            &vg,
            port(b_id, PortKind::Trigger, 0),
            port(a_id, PortKind::Event, 0),
        )
        .unwrap()
        .unwrap();

        match action {
            GraphUiAction::EventConnectionChanged {
                event_node_id,
                event_idx,
                subscriber,
                change,
            } => {
                assert_eq!(event_node_id, a_id);
                assert_eq!(event_idx, 0);
                assert_eq!(subscriber, b_id);
                assert_eq!(change, EventSubscriberChange::Added);
            }
            other => panic!("expected EventConnectionChanged, got {other:?}"),
        }
    }

    // --- round-trip: apply(build_data_connection_action) matches the expected graph

    // --- handle_idle (free function, pure state machine transition) -----

    #[test]
    fn handle_idle_primary_not_down_keeps_idle() {
        let mut interaction = Interaction::default();
        handle_idle(
            &mut interaction,
            Pos2::new(10.0, 20.0),
            false,
            true,
            PortInteractCommand::None,
        );
        assert!(interaction.is_idle());
    }

    #[test]
    fn handle_idle_drag_start_transitions_to_dragging_connection() {
        let mut interaction = Interaction::default();
        let port_info = PortInfo {
            port: port(NodeId::unique(), PortKind::Output, 0),
            center: Pos2::new(1.0, 2.0),
        };
        handle_idle(
            &mut interaction,
            Pos2::new(10.0, 20.0),
            true,
            true, // on background, but DragStart wins
            PortInteractCommand::DragStart(port_info),
        );
        assert!(interaction.is_dragging_connection());
    }

    #[test]
    fn handle_idle_background_click_transitions_to_breaking() {
        let mut interaction = Interaction::default();
        handle_idle(
            &mut interaction,
            Pos2::new(10.0, 20.0),
            true,
            true,
            PortInteractCommand::None,
        );
        assert!(interaction.is_breaking());
    }

    #[test]
    fn handle_idle_port_hover_without_drag_start_stays_idle() {
        let mut interaction = Interaction::default();
        let port_info = PortInfo {
            port: port(NodeId::unique(), PortKind::Input, 0),
            center: Pos2::ZERO,
        };
        handle_idle(
            &mut interaction,
            Pos2::new(10.0, 20.0),
            true,
            false, // not on background
            PortInteractCommand::Hover(port_info),
        );
        // Not on background, no drag start — shouldn't transition.
        assert!(interaction.is_idle());
    }

    #[test]
    fn build_data_connection_action_action_applies_to_bound_binding() {
        let a = make_node_with(1, 0);
        let b = make_node_with(1, 0);
        let a_id = a.id;
        let b_id = b.id;
        let mut vg = view_graph_with_nodes(vec![a, b]);

        let action = build_data_connection_action(
            &vg,
            port(a_id, PortKind::Input, 0),
            port(b_id, PortKind::Output, 0),
        )
        .unwrap();

        action.apply(&mut vg);

        let a_input = &vg.graph.by_id(&a_id).unwrap().inputs[0];
        match &a_input.binding {
            Binding::Bind(addr) => {
                assert_eq!(addr.target_id, b_id);
                assert_eq!(addr.port_idx, 0);
            }
            other => panic!("expected Bind, got {other:?}"),
        }
    }
}
