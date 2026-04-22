//! Top-level graph view. Owns the per-frame render pipeline
//! (content → overlays → zoom/pan) and the interaction state machine.
//!
//! The heavy logic lives in submodules:
//! - `connections` — drag/snap/commit for data + event wires, breaker
//! - `overlays`    — button bars, new-node popup, const-bind routing
//! - `pan_zoom`    — viewport transforms and fit/reset targets

use bumpalo::Bump;
use common::BoolExt;
use eframe::egui;
use egui::{Pos2, Rect, Response, Sense, StrokeKind, Vec2};
use scenarium::graph::NodeId;

use crate::app_data::AppData;
use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::connection_ui::ConnectionUi;
use crate::gui::frame_output::FrameOutput;
use crate::gui::gesture::Gesture;
use crate::gui::graph_background::GraphBackgroundRenderer;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::GraphLayout;
use crate::gui::new_node_ui::NewNodeUi;
use crate::gui::node_details_ui::NodeDetailsUi;
use crate::gui::node_ui::NodeUi;
use crate::input::InputSnapshot;
use crate::model;

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
                "connection would create a cycle between {input_node_id} and {output_node_id}"
            ),
        }
    }
}

#[derive(Debug, Default)]
pub struct GraphUi {
    /// Centralized interaction state machine.
    gesture: Gesture,
    connections: ConnectionUi,
    graph_layout: GraphLayout,
    node_ui: NodeUi,
    dots_background: GraphBackgroundRenderer,
    new_node_ui: NewNodeUi,
    node_details_ui: NodeDetailsUi,
    output: FrameOutput,
}

impl GraphUi {
    pub fn output(&mut self) -> &mut FrameOutput {
        &mut self.output
    }

    pub fn cancel_gesture(&mut self) {
        self.gesture.cancel();
    }

    /// Per-frame entry point. The body splits into three ordered phases:
    ///   1. content — layout, background, connections, nodes
    ///   2. overlays — buttons, details panel, new-node popup
    ///   3. zoom/pan — only when no overlay is hovered
    pub fn render(
        &mut self,
        gui: &mut Gui<'_>,
        app_data: &mut AppData,
        input: &InputSnapshot,
        arena: &Bump,
    ) {
        self.output.clear();

        if input.cancel_requested() {
            self.cancel_gesture();
        }

        let rect = self.draw_background_frame(gui);

        gui.scope(StableId::new("graph_ui"))
            .max_rect(rect)
            .show(|gui| {
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

                // Render nodes first so their widgets (body, cache button,
                // remove button, ports) register before we read the
                // background's click state. Otherwise the background —
                // the only widget in scope at this point — would claim
                // clicks that should have gone to nodes, triggering a
                // spurious deselect + cancel_gesture.
                self.render_content(gui, &ctx, input, &background_response, pointer_pos);

                if background_response.clicked() {
                    self.handle_background_click(&ctx);
                }

                let overlay_hovered = self.render_overlays(
                    gui,
                    &mut ctx,
                    input,
                    pointer_pos,
                    &background_response,
                    arena,
                );

                if !overlay_hovered && (self.gesture.is_idle() || self.gesture.is_panning()) {
                    self.update_zoom_and_pan(gui, input, &ctx, &background_response, pointer_pos);
                }
            });
    }

    // ------------------------------------------------------------------------
    // Render phases
    // ------------------------------------------------------------------------

    /// Phase 1 — graph content rendered at the graph's zoom scale.
    fn render_content(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext<'_>,
        input: &InputSnapshot,
        background_response: &Response,
        pointer_pos: Option<Pos2>,
    ) {
        gui.with_scale(ctx.view_graph.scale, |gui| {
            // Refresh galleys first so every subsequent call site
            // can assume galleys exist for every view-node.
            self.graph_layout.refresh_galleys(gui, ctx);

            // Interact with fresh body rects — layouts are computed on
            // demand from the current gesture offset, so this frame's
            // drag delta accumulates into the gesture before rendering
            // reads it again. No stale-rect flash, no dual pass.
            self.node_ui.handle_node_interactions(
                gui,
                ctx,
                &self.graph_layout,
                &mut self.output,
                &mut self.gesture,
            );

            self.dots_background.render(gui, ctx);
            self.render_connections(gui, ctx);

            // Overlay that swallows background click-through for active
            // gestures. Registered HERE — before ports — so later-registered
            // port widgets keep higher egui z-order and their click/drag
            // responses still fire through. See `maybe_capture_overlay`.
            Self::maybe_capture_overlay(gui, &self.gesture);

            let nodes_result = self.node_ui.render_nodes(
                gui,
                ctx,
                &self.graph_layout,
                &mut self.output,
                &self.gesture,
            );

            // Surface render-time removal intents as actions. The mutation
            // itself happens in `NodeRemoved::apply` during `handle_actions`.
            for node_id in &nodes_result.removed_nodes {
                let action = ctx.view_graph.removal_action(node_id);
                self.output.add_action(action);
            }

            if let Some(pointer_pos) = pointer_pos {
                self.process_connections(
                    input,
                    ctx,
                    background_response,
                    pointer_pos,
                    nodes_result.port_cmd,
                    &nodes_result.broken_nodes,
                );
            }
        });
    }

    /// Phase 2 — overlay UI. Returns `true` if any overlay is hovered,
    /// which suppresses zoom/pan input in phase 3.
    fn render_overlays(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext<'_>,
        input: &InputSnapshot,
        pointer_pos: Option<Pos2>,
        background_response: &Response,
        arena: &Bump,
    ) -> bool {
        let buttons = self.render_buttons(gui, ctx.autorun);

        if buttons.reset_view {
            self.emit_zoom_pan(ctx.view_graph, Vec2::ZERO, 1.0);
        }
        if buttons.view_selected
            && let Some((scale, pan)) = view_selected_node_target(gui, ctx, &self.graph_layout)
        {
            self.emit_zoom_pan(ctx.view_graph, pan, scale);
        }
        if buttons.fit_all {
            let (scale, pan) = fit_all_nodes_target(gui, ctx, &self.graph_layout);
            self.emit_zoom_pan(ctx.view_graph, pan, scale);
        }

        let mut hovered = buttons.response.hovered();
        hovered |= self
            .node_details_ui
            .show(gui, ctx, &mut self.output)
            .hovered();
        hovered |=
            self.handle_new_node_popup(gui, input, ctx, pointer_pos, background_response, arena);
        hovered
    }

    // ------------------------------------------------------------------------
    // Background
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
}

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
    use crate::gui::connection_ui::PortKind;
    use crate::gui::graph_layout::{PortInfo, PortRef};
    use crate::gui::node_ui::PortInteractCommand;
    use crate::model::EventSubscriberChange;
    use crate::model::graph_ui_action::GraphUiAction;
    use scenarium::function::FuncId;
    use scenarium::graph::{Event, Input, Node, NodeBehavior};
    use scenarium::prelude::{Binding, PortAddress};

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

    // --- handle_idle (pure state-machine transition) ----------------------------

    #[test]
    fn handle_idle_primary_not_down_keeps_idle() {
        let mut interaction = Gesture::default();
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
        let mut interaction = Gesture::default();
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
        let mut interaction = Gesture::default();
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
        let mut interaction = Gesture::default();
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
}
