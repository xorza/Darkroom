//! Drag state machine + action emission for connections. Pure logic —
//! no egui runtime required, fully unit-testable. The renderer in
//! `mod.rs` calls these to interpret the gesture and emit actions
//! into the frame's output buffer; the actions themselves apply via
//! `Session::commit_actions`.

use egui::Pos2;
use scenarium::prelude::Binding;

use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::graph_ui::frame_output::FrameOutput;
use crate::gui::graph_ui::nodes::PortInteractCommand;
use crate::gui::graph_ui::port::{PortInfo, PortKind, PortRef};
use crate::model::EventSubscriberChange;
use crate::model::graph_ui_action::GraphUiAction;

use super::types::{ConnectionDrag, ConnectionDragUpdate, ConnectionKey};

/// Advances an in-flight connection drag based on the pointer and the latest
/// port interaction command. The drag lives inside
/// [`crate::gui::graph_ui::gesture::Gesture::DraggingConnection`] — this
/// is a free function so the caller owns both the drag and any transition
/// decision (e.g. cancelling the interaction on `Finished`).
pub(crate) fn advance_drag(
    drag: &mut ConnectionDrag,
    pointer_pos: Pos2,
    cmd: PortInteractCommand,
) -> ConnectionDragUpdate {
    drag.current_pos = pointer_pos;

    match cmd {
        PortInteractCommand::None => {
            drag.end_port = None;
            ConnectionDragUpdate::InProgress
        }
        PortInteractCommand::DragStart(_) => {
            panic!("advance_drag received DragStart — caller must start the interaction instead")
        }
        PortInteractCommand::Hover(port_info) => {
            try_snap_to_port(drag, port_info);
            ConnectionDragUpdate::InProgress
        }
        PortInteractCommand::DragStop => finish_drag(drag),
        PortInteractCommand::Click(port_info) => {
            if try_snap_to_port(drag, port_info) {
                finish_drag(drag)
            } else {
                ConnectionDragUpdate::Finished
            }
        }
    }
}

/// Emits the undoable action that clears the connection identified by `key`.
///
/// Used by the breaker tool and double-click deletion. The actual binding /
/// subscriber mutation happens via the emitted action's `apply` in
/// `commit_actions` — nothing here writes to the graph.
pub(crate) fn disconnect_connection(
    key: ConnectionKey,
    ctx: &GraphContext,
    output: &mut FrameOutput,
) {
    match key {
        ConnectionKey::Input {
            input_node_id,
            input_idx,
        } => {
            let node = ctx.view_graph.graph.by_id(&input_node_id).unwrap();
            let before = node.inputs[input_idx].binding.clone();
            if matches!(before, Binding::None) {
                return;
            }
            output.add_action(GraphUiAction::InputChanged {
                node_id: input_node_id,
                input_idx,
                before,
                after: Binding::None,
            });
        }
        ConnectionKey::Event {
            event_node_id,
            event_idx,
            trigger_node_id,
        } => {
            let node = ctx.view_graph.graph.by_id(&event_node_id).unwrap();
            if !node.events[event_idx]
                .subscribers
                .contains(&trigger_node_id)
            {
                return;
            }
            output.add_action(GraphUiAction::EventConnectionChanged {
                event_node_id,
                event_idx,
                subscriber: trigger_node_id,
                change: EventSubscriberChange::Removed,
            });
        }
    }
}

pub(super) fn apply_connection_deletions(
    deletions: Vec<ConnectionKey>,
    ctx: &GraphContext,
    output: &mut FrameOutput,
) {
    for key in deletions {
        disconnect_connection(key, ctx, output);
    }
}

pub(super) fn order_ports(port_a: PortRef, port_b: PortRef) -> (PortRef, PortRef) {
    match (port_a.kind, port_b.kind) {
        (PortKind::Output, PortKind::Input) => (port_b, port_a),
        (PortKind::Input, PortKind::Output) => (port_a, port_b),
        (PortKind::Event, PortKind::Trigger) => (port_b, port_a),
        (PortKind::Trigger, PortKind::Event) => (port_a, port_b),
        _ => unreachable!("ports must be of opposite types"),
    }
}

fn try_snap_to_port(drag: &mut ConnectionDrag, port_info: PortInfo) -> bool {
    drag.end_port = None;
    if drag.start_port.kind.opposite() == port_info.port.kind {
        drag.end_port = Some(port_info.port);
        drag.current_pos = port_info.center;
        true
    } else {
        false
    }
}

fn finish_drag(drag: &ConnectionDrag) -> ConnectionDragUpdate {
    if let Some(end_port) = drag.end_port {
        let (input_port, output_port) = order_ports(drag.start_port, end_port);
        ConnectionDragUpdate::FinishedWith {
            input_port,
            output_port,
        }
    } else {
        match drag.start_port.kind {
            PortKind::Input => ConnectionDragUpdate::FinishedWithEmptyOutput {
                input_port: drag.start_port,
            },
            PortKind::Output => ConnectionDragUpdate::FinishedWithEmptyInput {
                output_port: drag.start_port,
            },
            _ => ConnectionDragUpdate::Finished,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================
//
// Exercise the drag state machine plus the disconnect-action emission.
// No egui runtime required — `advance_drag` is a pure transition over
// `ConnectionDrag`, and `disconnect_connection` reads `&GraphContext`
// and emits actions into a buffer.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::graph_ui::port::PortInfo;
    use crate::model::{ViewGraph, ViewNode};
    use scenarium::function::FuncId;
    use scenarium::graph::{Event, Input, Node, NodeBehavior, NodeId, PortAddress};
    use scenarium::prelude::FuncLib;

    fn port_info(node_id: NodeId, kind: PortKind, port_idx: usize) -> PortInfo {
        PortInfo {
            port: PortRef {
                node_id,
                kind,
                port_idx,
            },
            center: Pos2::ZERO,
        }
    }

    // --- advance_drag ----------------------------------------------------

    #[test]
    fn advance_drag_none_clears_end_port_and_continues() {
        let start = port_info(NodeId::unique(), PortKind::Output, 0);
        let mut drag = ConnectionDrag::new(start);
        drag.end_port = Some(port_info(NodeId::unique(), PortKind::Input, 0).port);

        let update = advance_drag(&mut drag, Pos2::new(5.0, 6.0), PortInteractCommand::None);

        assert!(matches!(update, ConnectionDragUpdate::InProgress));
        assert!(drag.end_port.is_none());
        assert_eq!(drag.current_pos, Pos2::new(5.0, 6.0));
    }

    #[test]
    fn advance_drag_hover_snaps_to_compatible_port() {
        let start = port_info(NodeId::unique(), PortKind::Output, 0);
        let target = port_info(NodeId::unique(), PortKind::Input, 1);
        let mut drag = ConnectionDrag::new(start);

        let update = advance_drag(
            &mut drag,
            Pos2::new(3.0, 4.0),
            PortInteractCommand::Hover(target),
        );

        assert!(matches!(update, ConnectionDragUpdate::InProgress));
        assert_eq!(drag.end_port.unwrap(), target.port);
    }

    #[test]
    fn advance_drag_hover_rejects_same_kind_port() {
        // Output → Output is invalid.
        let start = port_info(NodeId::unique(), PortKind::Output, 0);
        let same_kind = port_info(NodeId::unique(), PortKind::Output, 1);
        let mut drag = ConnectionDrag::new(start);

        let update = advance_drag(&mut drag, Pos2::ZERO, PortInteractCommand::Hover(same_kind));

        assert!(matches!(update, ConnectionDragUpdate::InProgress));
        assert!(drag.end_port.is_none(), "incompatible port must not snap");
    }

    #[test]
    fn advance_drag_stop_with_snap_returns_finished_with() {
        let start = port_info(NodeId::unique(), PortKind::Output, 0);
        let mut drag = ConnectionDrag::new(start);
        drag.end_port = Some(port_info(NodeId::unique(), PortKind::Input, 0).port);

        let update = advance_drag(&mut drag, Pos2::ZERO, PortInteractCommand::DragStop);
        assert!(matches!(update, ConnectionDragUpdate::FinishedWith { .. }));
    }

    #[test]
    fn advance_drag_stop_without_snap_from_input_asks_for_output_source() {
        let start = port_info(NodeId::unique(), PortKind::Input, 0);
        let mut drag = ConnectionDrag::new(start);

        let update = advance_drag(&mut drag, Pos2::ZERO, PortInteractCommand::DragStop);

        match update {
            ConnectionDragUpdate::FinishedWithEmptyOutput { input_port } => {
                assert_eq!(input_port, start.port);
            }
            other => panic!("expected FinishedWithEmptyOutput, got {other:?}"),
        }
    }

    #[test]
    fn advance_drag_stop_without_snap_from_output_asks_for_input_target() {
        let start = port_info(NodeId::unique(), PortKind::Output, 0);
        let mut drag = ConnectionDrag::new(start);

        let update = advance_drag(&mut drag, Pos2::ZERO, PortInteractCommand::DragStop);

        match update {
            ConnectionDragUpdate::FinishedWithEmptyInput { output_port } => {
                assert_eq!(output_port, start.port);
            }
            other => panic!("expected FinishedWithEmptyInput, got {other:?}"),
        }
    }

    // --- disconnect_connection ------------------------------------------

    fn make_node(input_count: usize, event_count: usize) -> Node {
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

    fn with_ctx<R>(vg: &ViewGraph, f: impl FnOnce(&GraphContext<'_>) -> R) -> R {
        let func_lib = FuncLib::default();
        let ctx = GraphContext {
            func_lib: &func_lib,
            view_graph: vg,
            execution_stats: None,
            exec_info_index: crate::model::NodeExecutionIndex::new(None),
            autorun: false,
        };
        f(&ctx)
    }

    #[test]
    fn disconnect_connection_input_emits_clear_action() {
        let source = make_node(0, 0);
        let mut target = make_node(1, 0);
        let source_id = source.id;
        let target_id = target.id;
        target.inputs[0].binding = Binding::Bind(PortAddress {
            target_id: source_id,
            port_idx: 0,
        });

        let mut vg = ViewGraph::default();
        for n in [source, target] {
            let vn = ViewNode {
                id: n.id,
                pos: Pos2::ZERO,
            };
            vg.view_nodes.add(vn);
            vg.graph.add(n);
        }

        let mut buf = FrameOutput::default();
        with_ctx(&vg, |ctx| {
            disconnect_connection(
                ConnectionKey::Input {
                    input_node_id: target_id,
                    input_idx: 0,
                },
                ctx,
                &mut buf,
            );
        });

        let actions = buf.actions();
        assert_eq!(actions.len(), 1, "expected exactly one emitted action");
        match &actions[0] {
            GraphUiAction::InputChanged { after, node_id, .. } => {
                assert_eq!(*node_id, target_id);
                assert!(matches!(after, Binding::None));
            }
            other => panic!("expected InputChanged, got {other:?}"),
        }
    }

    #[test]
    fn disconnect_connection_input_is_noop_when_already_none() {
        let node = make_node(1, 0);
        let node_id = node.id;
        let mut vg = ViewGraph::default();
        vg.view_nodes.add(ViewNode {
            id: node_id,
            pos: Pos2::ZERO,
        });
        vg.graph.add(node);

        let mut buf = FrameOutput::default();
        with_ctx(&vg, |ctx| {
            disconnect_connection(
                ConnectionKey::Input {
                    input_node_id: node_id,
                    input_idx: 0,
                },
                ctx,
                &mut buf,
            );
        });
        assert!(buf.actions().is_empty());
    }

    #[test]
    fn disconnect_connection_event_emits_removed_action() {
        let mut emitter = make_node(0, 1);
        let subscriber = make_node(0, 0);
        let emitter_id = emitter.id;
        let subscriber_id = subscriber.id;
        emitter.events[0].subscribers.push(subscriber_id);

        let mut vg = ViewGraph::default();
        for n in [emitter, subscriber] {
            let vn = ViewNode {
                id: n.id,
                pos: Pos2::ZERO,
            };
            vg.view_nodes.add(vn);
            vg.graph.add(n);
        }

        let mut buf = FrameOutput::default();
        with_ctx(&vg, |ctx| {
            disconnect_connection(
                ConnectionKey::Event {
                    event_node_id: emitter_id,
                    event_idx: 0,
                    trigger_node_id: subscriber_id,
                },
                ctx,
                &mut buf,
            );
        });

        let actions = buf.actions();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            GraphUiAction::EventConnectionChanged {
                event_node_id,
                subscriber,
                change,
                ..
            } => {
                assert_eq!(*event_node_id, emitter_id);
                assert_eq!(*subscriber, subscriber_id);
                assert_eq!(*change, EventSubscriberChange::Removed);
            }
            other => panic!("expected EventConnectionChanged, got {other:?}"),
        }
    }

    #[test]
    fn disconnect_connection_event_is_noop_when_not_subscribed() {
        let emitter = make_node(0, 1);
        let subscriber = make_node(0, 0);
        let emitter_id = emitter.id;
        let subscriber_id = subscriber.id;

        let mut vg = ViewGraph::default();
        for n in [emitter, subscriber] {
            let vn = ViewNode {
                id: n.id,
                pos: Pos2::ZERO,
            };
            vg.view_nodes.add(vn);
            vg.graph.add(n);
        }

        let mut buf = FrameOutput::default();
        with_ctx(&vg, |ctx| {
            disconnect_connection(
                ConnectionKey::Event {
                    event_node_id: emitter_id,
                    event_idx: 0,
                    trigger_node_id: subscriber_id,
                },
                ctx,
                &mut buf,
            );
        });
        assert!(buf.actions().is_empty());
    }
}
