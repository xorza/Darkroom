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
use crate::gui::connection_ui::{
    BrokeItem, ConnectionDragUpdate, PortKind, advance_drag, disconnect_connection,
};
use crate::gui::gesture::Gesture;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::PortRef;
use crate::gui::graph_ui::GraphUi;
use crate::gui::node_ui::PortInteractCommand;
use crate::gui::widgets::HitRegion;
use crate::input::InputSnapshot;
use crate::model::EventSubscriberChange;
use crate::model::graph_ui_action::GraphUiAction;

impl GraphUi {
    pub(super) fn handle_background_click(&mut self, ctx: &GraphContext<'_>) {
        self.cancel_gesture();

        if ctx.view_graph.selected_node_id.is_some() {
            let before = ctx.view_graph.selected_node_id;
            // Emit-action-only: selected_node_id is mutated by
            // `NodeSelected::apply` in handle_actions, not here.
            self.output.add_action(GraphUiAction::NodeSelected {
                before,
                after: None,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn process_connections(
        &mut self,
        input: &InputSnapshot,
        ctx: &GraphContext<'_>,
        background_response: &Response,
        pointer_pos: Pos2,
        port_interact_cmd: PortInteractCommand,
        broken_nodes: &[NodeId],
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
                    self.apply_breaker_results(ctx, broken_nodes);
                    self.gesture.cancel();
                }
            }
            Gesture::DraggingConnection(drag) => {
                let result = advance_drag(drag, pointer_pos, port_interact_cmd);
                self.handle_drag_result(ctx, pointer_pos, result);
            }
        }
    }

    /// Registers a full-area interact widget during connection-dragging
    /// or breaker modes so stray clicks can't fall through to the
    /// background (which would trigger background-click handlers).
    /// Called from `render` in phase 1, between `render_connections`
    /// and `render_nodes`, so that ports — registered afterwards — keep
    /// a higher egui z-order than this overlay and still receive their
    /// click/drag events. That subtlety is why this lives here and not
    /// inside `process_connections`.
    pub(super) fn maybe_capture_overlay(gui: &mut Gui<'_>, gesture: &Gesture) {
        if matches!(
            gesture,
            Gesture::BreakingConnections(_) | Gesture::DraggingConnection(_)
        ) {
            let rect = gui.rect;
            HitRegion::new(StableId::new("temp_overlay_background"))
                .rect(rect)
                .sense(Sense::all())
                .show(gui);
        }
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
                    disconnect_connection(key, ctx, &mut self.output);
                }
                BrokeItem::Node(node_id) => {
                    let action = ctx.view_graph.removal_action(&node_id);
                    self.output.add_action(action);
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
                self.apply_connection(ctx, input_port, output_port);
                self.gesture.cancel();
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
                match build_data_connection_action(ctx.view_graph, input_port, output_port) {
                    Ok(action) => self.output.add_action(action),
                    Err(err) => self.output.add_error(err),
                }
            }
            PortKind::Event => {
                match build_event_connection_action(ctx.view_graph, input_port, output_port) {
                    Ok(Some(action)) => self.output.add_action(action),
                    Ok(None) => {}
                    Err(err) => self.output.add_error(err),
                }
            }
            _ => unreachable!(),
        }
    }

    pub(super) fn render_connections(&mut self, gui: &mut Gui<'_>, ctx: &GraphContext<'_>) {
        self.connections.render(
            gui,
            ctx,
            &self.graph_layout,
            &self.gesture,
            &mut self.output,
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
pub(super) fn handle_idle(
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
pub(super) fn build_data_connection_action(
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
pub(super) fn build_event_connection_action(
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
