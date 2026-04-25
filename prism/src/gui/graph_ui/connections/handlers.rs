//! Connection-related gesture: drag → snap → commit pipeline for
//! data (input-binding) and event (trigger) connections, plus the
//! breaker tool's deletion pass.
//!
//! Every function here emits `GraphUiAction`s into the gesture
//! buffer; `ViewGraph` is never mutated directly.

use egui::{Pos2, Response, Sense};
use scenarium::graph::NodeId;
use scenarium::prelude::{Binding, PortAddress};

use crate::common::StableId;
use crate::gui::Gui;
use crate::gui::graph_ui::GraphUi;
use crate::gui::graph_ui::connections::actions::{advance_drag, disconnect_connection};
use crate::gui::graph_ui::connections::{BrokeItem, ConnectionDragUpdate};
use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::graph_ui::frame_output::FrameOutput;
use crate::gui::graph_ui::gesture::Gesture;
use crate::gui::graph_ui::nodes::PortInteractCommand;
use crate::gui::graph_ui::port::{PortKind, PortRef};
use crate::gui::widgets::HitRegion;
use crate::input::InputSnapshot;
use crate::model::EventSubscriberChange;
use crate::model::graph_ui_action::GraphUiAction;

impl GraphUi {
    pub(in crate::gui::graph_ui) fn handle_background_click(
        &mut self,
        ctx: &GraphContext<'_>,
        output: &mut FrameOutput,
    ) {
        self.cancel_gesture();

        if ctx.view_graph.selected_node_id.is_some() {
            let before = ctx.view_graph.selected_node_id;
            // Emit-action-only: selected_node_id is mutated by
            // `NodeSelected::apply` in commit_actions, not here.
            output.add_action(GraphUiAction::NodeSelected {
                before,
                after: None,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::gui::graph_ui) fn process_connections(
        &mut self,
        input: &InputSnapshot,
        ctx: &GraphContext<'_>,
        background_response: &Response,
        pointer_pos: Pos2,
        port_interact_cmd: PortInteractCommand,
        broken_nodes: &[NodeId],
        output: &mut FrameOutput,
    ) {
        let primary_down = input.primary_pressed || input.primary_down;

        match &mut self.gesture {
            // Node drag and view pan advance through egui response events
            // elsewhere in the frame; process_connections has nothing to
            // add for them.
            Gesture::Panning | Gesture::DraggingNode(_) => {}
            Gesture::Idle => {
                let pointer_on_background =
                    background_response.hovered() && !self.connections.any_hovered();
                handle_idle(
                    &mut self.gesture,
                    pointer_pos,
                    primary_down,
                    pointer_on_background,
                    port_interact_cmd,
                );
            }
            Gesture::BreakingConnections(breaker) => {
                if primary_down {
                    breaker.add_point(pointer_pos);
                } else {
                    // Breaker released — collect results, then cancel.
                    self.apply_breaker_results(ctx, broken_nodes, output);
                    self.gesture.cancel();
                }
            }
            Gesture::DraggingConnection(drag) => {
                let result = advance_drag(drag, pointer_pos, port_interact_cmd);
                self.handle_drag_result(ctx, pointer_pos, result, output);
            }
        }
    }

    /// Collects all items hit by the breaker (connections, const bindings, nodes)
    /// and applies the corresponding removals in one pass.
    fn apply_breaker_results(
        &mut self,
        ctx: &GraphContext<'_>,
        broken_nodes: &[NodeId],
        output: &mut FrameOutput,
    ) {
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
                    disconnect_connection(key, ctx, output);
                }
                BrokeItem::Node(node_id) => {
                    let action = GraphUiAction::node_removal(ctx.view_graph, &node_id);
                    output.add_action(action);
                }
            }
        }
    }

    fn handle_drag_result(
        &mut self,
        ctx: &GraphContext<'_>,
        pointer_pos: Pos2,
        result: ConnectionDragUpdate,
        output: &mut FrameOutput,
    ) {
        match result {
            ConnectionDragUpdate::InProgress => {}
            ConnectionDragUpdate::Finished => {
                self.gesture.cancel();
            }
            ConnectionDragUpdate::FinishedWithEmptyOutput { input_port } => {
                assert_eq!(input_port.kind, PortKind::Input);
                // NB: gesture stays in DraggingConnection so the ports
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
                self.apply_connection(ctx, input_port, output_port, output);
                self.gesture.cancel();
            }
        }
    }

    fn apply_connection(
        &mut self,
        ctx: &GraphContext<'_>,
        input_port: PortRef,
        output_port: PortRef,
        output: &mut FrameOutput,
    ) {
        assert_eq!(input_port.kind, output_port.kind.opposite());

        match output_port.kind {
            PortKind::Output => {
                match build_data_connection_action(ctx.view_graph, input_port, output_port) {
                    Ok(action) => output.add_action(action),
                    Err(err) => output.add_error(err),
                }
            }
            PortKind::Event => {
                match build_event_connection_action(ctx.view_graph, input_port, output_port) {
                    Ok(Some(action)) => output.add_action(action),
                    Ok(None) => {}
                    Err(err) => output.add_error(err),
                }
            }
            _ => unreachable!(),
        }
    }

    /// Renders existing connections plus, during a connection-related
    /// gesture, a full-area hit region that swallows stray background
    /// clicks. The hit region is registered *here* — before nodes —
    /// so port widgets registered later keep higher egui z-order and
    /// still receive their click/drag events.
    pub(in crate::gui::graph_ui) fn render_connections(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext<'_>,
        output: &mut FrameOutput,
    ) {
        self.connections.render(
            gui,
            ctx,
            &self.graph_layout,
            &self.gesture,
            output,
            self.gesture.breaker(),
        );

        match &mut self.gesture {
            Gesture::BreakingConnections(breaker) => {
                breaker.show(gui);
            }
            Gesture::DraggingConnection(drag) => {
                self.connections
                    .render_temp_connection(gui, ctx, &self.graph_layout, drag);
            }
            _ => {}
        }

        if matches!(
            self.gesture,
            Gesture::BreakingConnections(_) | Gesture::DraggingConnection(_)
        ) {
            let rect = gui.rect;
            HitRegion::new(StableId::new("temp_overlay_background"))
                .rect(rect)
                .sense(Sense::all())
                .show(gui);
        }
    }
}

// ============================================================================
// Connection errors
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionError {
    CycleDetected {
        input_node_id: NodeId,
        output_node_id: NodeId,
    },
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::CycleDetected {
                input_node_id,
                output_node_id,
            } => write!(
                f,
                "connection would create a cycle between {input_node_id} and {output_node_id}"
            ),
        }
    }
}

// ============================================================================
// Pure helpers (unit-testable — no egui runtime).
// ============================================================================

/// Idle-state transitions driven by primary-button pressure plus a port
/// gesture command. Kept as a free function so it can borrow
/// `gesture` mutably without the enclosing `process_connections`
/// match needing to drop its discriminant borrow.
pub(in crate::gui::graph_ui) fn handle_idle(
    gesture: &mut Gesture,
    pointer_pos: Pos2,
    primary_down: bool,
    pointer_on_background: bool,
    port_interact_cmd: PortInteractCommand,
) {
    if !primary_down {
        return;
    }

    if let PortInteractCommand::DragStart(port_info) = port_interact_cmd {
        gesture.start_dragging(port_info);
    } else if pointer_on_background {
        gesture.start_breaking(pointer_pos);
    }
}

/// Builds the `InputChanged` action that would bind `input_port` to
/// `output_port`. No mutation — `apply` handles it. Always produces
/// an action; re-binding to the same source is a harmless overwrite.
pub(in crate::gui::graph_ui) fn build_data_connection_action(
    view_graph: &crate::model::ViewGraph,
    input_port: PortRef,
    output_port: PortRef,
) -> Result<GraphUiAction, ConnectionError> {
    if input_port.node_id == output_port.node_id {
        return Err(ConnectionError::CycleDetected {
            input_node_id: input_port.node_id,
            output_node_id: output_port.node_id,
        });
    }

    let dependents = view_graph.graph.dependent_nodes(&input_port.node_id);
    if dependents.contains(&output_port.node_id) {
        return Err(ConnectionError::CycleDetected {
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
/// Returns `Ok(None)` when the subscriber is already present —
/// event subscriptions are list membership, unlike data bindings
/// which are overwrites.
pub(in crate::gui::graph_ui) fn build_event_connection_action(
    view_graph: &crate::model::ViewGraph,
    input_port: PortRef,
    output_port: PortRef,
) -> Result<Option<GraphUiAction>, ConnectionError> {
    assert_eq!(input_port.kind, PortKind::Trigger);
    assert_eq!(output_port.kind, PortKind::Event);

    if input_port.node_id == output_port.node_id {
        return Err(ConnectionError::CycleDetected {
            input_node_id: input_port.node_id,
            output_node_id: output_port.node_id,
        });
    }

    let output_node = view_graph
        .graph
        .by_id(&output_port.node_id)
        .expect("event connection output node must exist");
    assert!(
        view_graph.graph.by_id(&input_port.node_id).is_some(),
        "event connection input node must exist"
    );
    assert!(
        output_port.port_idx < output_node.events.len(),
        "event index out of range for build_event_connection_action"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::graph_ui::port::PortInfo;
    use crate::model;
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
        assert!(matches!(err, ConnectionError::CycleDetected { .. }));
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
        assert!(matches!(err, ConnectionError::CycleDetected { .. }));
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
        assert!(matches!(err, ConnectionError::CycleDetected { .. }));
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
        assert!(matches!(interaction, Gesture::DraggingConnection(_)));
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
        assert!(matches!(interaction, Gesture::BreakingConnections(_)));
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
