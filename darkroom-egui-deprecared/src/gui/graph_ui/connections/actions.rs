//! Drag state machine + action emission for connections. Pure logic —
//! no egui runtime required, fully unit-testable. The renderer in
//! `mod.rs` calls these to interpret the gesture and emit actions
//! into the frame's output buffer; the actions themselves apply via
//! `Session::commit_actions`.

use egui::Pos2;
use scenarium::prelude::Binding;

use crate::gui::graph_ui::ctx::GraphContext;
use crate::gui::graph_ui::nodes::PortInteractCommand;
use crate::gui::graph_ui::port::{PortInfo, PortKind, PortRef};
use crate::session::output::FrameOutput;

use crate::model::Intent;

use super::types::{ConnectionDrag, ConnectionDragUpdate, ConnectionKey, ConnectionPair};

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
            if matches!(node.inputs[input_idx].binding, Binding::None) {
                return;
            }
            output.add_intent(Intent::SetInput {
                node_id: input_node_id,
                input_idx,
                to: Binding::None,
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
            output.add_intent(Intent::SetEventConnection {
                event_node_id,
                event_idx,
                subscriber: trigger_node_id,
                present: false,
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

/// Pair two opposite-kind ports into a typed [`ConnectionPair`]. Returns
/// `None` for any same-kind or cross-kind (data ↔ event) combination.
/// All four valid orderings are enumerated explicitly so adding a new
/// `PortKind` is a compile error here, not a runtime panic.
pub(super) fn pair_ports(port_a: PortRef, port_b: PortRef) -> Option<ConnectionPair> {
    use PortKind::*;
    match (port_a.kind, port_b.kind) {
        (Output, Input) => Some(ConnectionPair::Data {
            input: port_b,
            output: port_a,
        }),
        (Input, Output) => Some(ConnectionPair::Data {
            input: port_a,
            output: port_b,
        }),
        (Event, Trigger) => Some(ConnectionPair::Event {
            trigger: port_b,
            event: port_a,
        }),
        (Trigger, Event) => Some(ConnectionPair::Event {
            trigger: port_a,
            event: port_b,
        }),
        _ => None,
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
        // try_snap_to_port already verified the kinds are opposite, so
        // pair_ports cannot return None here.
        let pair = pair_ports(drag.start_port, end_port)
            .expect("snapped end port has been validated as opposite-kind");
        ConnectionDragUpdate::FinishedWith(pair)
    } else {
        match drag.start_port.kind {
            PortKind::Input => ConnectionDragUpdate::FinishedWithEmptyOutput {
                input_port: drag.start_port,
            },
            PortKind::Output => ConnectionDragUpdate::FinishedWithEmptyInput {
                output_port: drag.start_port,
            },
            // Trigger / Event drags don't open the new-node popup.
            PortKind::Trigger | PortKind::Event => ConnectionDragUpdate::Finished,
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
        let start_node = NodeId::unique();
        let end_node = NodeId::unique();
        let start = port_info(start_node, PortKind::Output, 0);
        let mut drag = ConnectionDrag::new(start);
        let end = port_info(end_node, PortKind::Input, 1).port;
        drag.end_port = Some(end);

        let update = advance_drag(&mut drag, Pos2::ZERO, PortInteractCommand::DragStop);
        match update {
            ConnectionDragUpdate::FinishedWith(ConnectionPair::Data { input, output }) => {
                assert_eq!(input.node_id, end_node);
                assert_eq!(input.port_idx, 1);
                assert_eq!(output.node_id, start_node);
                assert_eq!(output.port_idx, 0);
            }
            other => panic!("expected FinishedWith(Data), got {other:?}"),
        }
    }

    #[test]
    fn advance_drag_event_pair_classifies_as_event() {
        let event_node = NodeId::unique();
        let trigger_node = NodeId::unique();
        let start = port_info(event_node, PortKind::Event, 2);
        let mut drag = ConnectionDrag::new(start);
        drag.end_port = Some(port_info(trigger_node, PortKind::Trigger, 0).port);

        let update = advance_drag(&mut drag, Pos2::ZERO, PortInteractCommand::DragStop);
        match update {
            ConnectionDragUpdate::FinishedWith(ConnectionPair::Event { trigger, event }) => {
                assert_eq!(trigger.node_id, trigger_node);
                assert_eq!(event.node_id, event_node);
                assert_eq!(event.port_idx, 2);
            }
            other => panic!("expected FinishedWith(Event), got {other:?}"),
        }
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

        let actions = buf.intents();
        assert_eq!(actions.len(), 1, "expected exactly one emitted action");
        match &actions[0] {
            Intent::SetInput { to, node_id, .. } => {
                assert_eq!(*node_id, target_id);
                assert!(matches!(to, Binding::None));
            }
            other => panic!("expected SetInput, got {other:?}"),
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
        assert!(buf.intents().is_empty());
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

        let actions = buf.intents();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            Intent::SetEventConnection {
                event_node_id,
                subscriber,
                present,
                ..
            } => {
                assert_eq!(*event_node_id, emitter_id);
                assert_eq!(*subscriber, subscriber_id);
                assert!(!*present);
            }
            other => panic!("expected SetEventConnection, got {other:?}"),
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
        assert!(buf.intents().is_empty());
    }
}
