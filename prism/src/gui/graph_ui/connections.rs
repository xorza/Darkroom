//! Connection-related interaction: drag → snap → commit pipeline for
//! data (input-binding) and event (trigger) connections, plus the
//! breaker tool's deletion pass.
//!
//! Every function here emits `GraphUiAction`s into the interaction
//! buffer; `ViewGraph` is never mutated directly. See Step 4.0 /
//! 4.1 in `REFACTOR_PLAN.md` for the contract.

use egui::{Pos2, Response, Sense};
use scenarium::graph::NodeId;
use scenarium::prelude::{Binding, PortAddress};

use crate::gui::Gui;
use crate::gui::connection_ui::{
    BrokeItem, ConnectionDragUpdate, PortKind, advance_drag, disconnect_connection,
};
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::PortRef;
use crate::gui::graph_ui::{Error, GraphUi};
use crate::gui::interaction_state::Interaction;
use crate::gui::node_ui::PortInteractCommand;
use crate::input::InputSnapshot;
use crate::model::EventSubscriberChange;
use crate::model::graph_ui_action::GraphUiAction;

impl GraphUi {
    pub(super) fn handle_background_click(&mut self, ctx: &GraphContext<'_>) {
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn process_connections(
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

    pub(super) fn render_connections(&mut self, gui: &mut Gui<'_>, ctx: &GraphContext<'_>) {
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
}

// ============================================================================
// Pure helpers (unit-testable — no egui runtime).
// ============================================================================

/// Idle-state transitions driven by primary-button pressure plus a port
/// interaction command. Kept as a free function so it can borrow
/// `interaction` mutably without the enclosing `process_connections`
/// match needing to drop its discriminant borrow.
pub(super) fn handle_idle(
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
pub(super) fn apply_data_connection(
    view_graph: &crate::model::ViewGraph,
    input_port: PortRef,
    output_port: PortRef,
) -> Result<GraphUiAction, Error> {
    if input_port.node_id == output_port.node_id {
        return Err(Error::CycleDetected {
            input_node_id: input_port.node_id,
            output_node_id: output_port.node_id,
        });
    }

    // Defensive: the drag that produced these ports could have been
    // invalidated by an intervening undo/redo. Bail gracefully instead
    // of panicking.
    if view_graph.graph.by_id(&input_port.node_id).is_none() {
        return Err(Error::StaleNode {
            node_id: input_port.node_id,
        });
    }
    if view_graph.graph.by_id(&output_port.node_id).is_none() {
        return Err(Error::StaleNode {
            node_id: output_port.node_id,
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
pub(super) fn apply_event_connection(
    view_graph: &crate::model::ViewGraph,
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

    // Defensive — see comment in `apply_data_connection`.
    if view_graph.graph.by_id(&input_port.node_id).is_none() {
        return Err(Error::StaleNode {
            node_id: input_port.node_id,
        });
    }
    let Some(output_node) = view_graph.graph.by_id(&output_port.node_id) else {
        return Err(Error::StaleNode {
            node_id: output_port.node_id,
        });
    };
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
