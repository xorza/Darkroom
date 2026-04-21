use common::key_index_vec::{CompactInsert, KeyIndexKey, KeyIndexVec};
use egui::{PointerButton, Pos2, Sense};
use scenarium::graph::{NodeId, PortAddress};
use scenarium::prelude::{Binding, ExecutionStats};
use scenarium::worker::EventRef;

use crate::common::connection_bezier::{ConnectionBezier, ConnectionBezierStyle};
use crate::gui::Gui;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};
use crate::gui::graph_ui_interaction::GraphUiInteraction;
use crate::gui::node_ui::PortInteractCommand;
use crate::model::EventSubscriberChange;
use crate::model::graph_ui_action::GraphUiAction;

// === Types ===

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConnectionKey {
    Input {
        input_node_id: NodeId,
        input_idx: usize,
    },
    Event {
        event_node_id: NodeId,
        event_idx: usize,
        trigger_node_id: NodeId,
    },
}

/// A single item broken by the connection breaker tool.
#[derive(Debug, Clone, Copy)]
pub(crate) enum BrokeItem {
    Connection(ConnectionKey),
    Node(NodeId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PortKind {
    Input,
    Output,
    Trigger,
    Event,
}

impl PortKind {
    pub fn opposite(&self) -> Self {
        match self {
            PortKind::Input => PortKind::Output,
            PortKind::Output => PortKind::Input,
            PortKind::Trigger => PortKind::Event,
            PortKind::Event => PortKind::Trigger,
        }
    }

    fn is_source(&self) -> bool {
        matches!(self, PortKind::Output | PortKind::Event)
    }
}

// === ConnectionDrag ===

#[derive(Debug)]
pub(crate) struct ConnectionDrag {
    pub(crate) start_port: PortInfo,
    pub(crate) end_port: Option<PortInfo>,
    pub(crate) current_pos: Pos2,
}

impl ConnectionDrag {
    pub(crate) fn new(port: PortInfo) -> Self {
        Self {
            current_pos: port.center,
            start_port: port,
            end_port: None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum ConnectionDragUpdate {
    InProgress,
    Finished,
    FinishedWithEmptyOutput {
        input_port: PortRef,
    },
    FinishedWithEmptyInput {
        output_port: PortRef,
    },
    FinishedWith {
        input_port: PortRef,
        output_port: PortRef,
    },
}

// === ConnectionCurve ===

#[derive(Debug)]
pub(crate) struct ConnectionCurve {
    pub(crate) key: ConnectionKey,
    pub(crate) broke: bool,
    pub(crate) hovered: bool,
    pub(crate) bezier: ConnectionBezier,
}

impl ConnectionCurve {
    pub(crate) fn new(key: ConnectionKey) -> Self {
        Self {
            key,
            broke: false,
            hovered: false,
            bezier: ConnectionBezier::default(),
        }
    }
}

impl KeyIndexKey<ConnectionKey> for ConnectionCurve {
    fn key(&self) -> &ConnectionKey {
        &self.key
    }
}

// === ConnectionUi ===

#[derive(Debug, Default)]
pub(crate) struct ConnectionUi {
    curves: KeyIndexVec<ConnectionKey, ConnectionCurve>,
    highlight_curves: KeyIndexVec<ConnectionKey, ConnectionCurve>,

    /// Cached mesh buffer for the in-flight drag preview — reused across
    /// frames while a drag is active. The drag data itself lives in
    /// [`crate::gui::interaction_state::Interaction::DraggingConnection`].
    temp_connection_bezier: ConnectionBezier,
}

impl ConnectionUi {
    pub(crate) fn render(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &GraphContext,
        graph_layout: &GraphLayout,
        ui_interaction: &mut GraphUiInteraction,
        breaker: Option<&ConnectionBreaker>,
    ) {
        let execution_stats = ctx.execution_stats;
        let mut curves = self.curves.compact_insert_start();
        let mut highlights = self.highlight_curves.compact_insert_start();
        let mut deletions: Vec<ConnectionKey> = Vec::new();

        for node_view in &ctx.view_graph.view_nodes {
            let node_id = node_view.id;
            let node = ctx.view_graph.graph.by_id(&node_id).unwrap();
            let node_layout = graph_layout.node_layout(&node_id);

            // Render data connections
            for (input_idx, input) in node.inputs.iter().enumerate() {
                let Binding::Bind(binding) = &input.binding else {
                    continue;
                };

                let key = ConnectionKey::Input {
                    input_node_id: node_id,
                    input_idx,
                };
                let output_layout = graph_layout.node_layout(&binding.target_id);
                let input_pos = node_layout.input_center(input_idx);
                let output_pos = output_layout.output_center(binding.port_idx);

                let is_missing = execution_stats.is_some_and(|stats| {
                    stats.missing_inputs.contains(&PortAddress {
                        target_id: node_id,
                        port_idx: input_idx,
                    })
                });

                if is_missing {
                    render_highlight_curve(gui, &mut highlights, key, output_pos, input_pos);
                }

                update_curve_interaction(
                    gui,
                    &mut curves,
                    &mut deletions,
                    key,
                    output_pos,
                    input_pos,
                    PortKind::Input,
                    breaker,
                );
            }

            // Render event connections
            for (event_idx, event) in node.events.iter().enumerate() {
                let event_pos = node_layout.event_center(event_idx);

                for &trigger_node_id in &event.subscribers {
                    let trigger_layout = graph_layout.node_layout(&trigger_node_id);
                    let trigger_pos = trigger_layout.trigger_center();

                    let key = ConnectionKey::Event {
                        event_node_id: node_id,
                        event_idx,
                        trigger_node_id,
                    };

                    let is_triggered = execution_stats.is_some_and(|stats| {
                        stats
                            .triggered_events
                            .contains(&EventRef { node_id, event_idx })
                    });

                    if is_triggered {
                        render_highlight_curve(gui, &mut highlights, key, event_pos, trigger_pos);
                    }

                    update_curve_interaction(
                        gui,
                        &mut curves,
                        &mut deletions,
                        key,
                        event_pos,
                        trigger_pos,
                        PortKind::Trigger,
                        breaker,
                    );
                }
            }
        }

        drop(curves);
        drop(highlights);

        apply_connection_deletions(deletions, ctx, ui_interaction);
    }

    /// Draws the in-flight connection preview for a drag owned by
    /// [`crate::gui::interaction_state::Interaction`].
    pub(crate) fn render_temp_connection(
        &mut self,
        gui: &mut Gui<'_>,
        graph_layout: &GraphLayout,
        drag: &mut ConnectionDrag,
    ) {
        // Update port positions from layout
        let start_layout = graph_layout.node_layout(&drag.start_port.port.node_id);
        drag.start_port.center = start_layout.port_center(&drag.start_port.port);

        if let Some(end) = drag.end_port.as_mut() {
            let end_layout = graph_layout.node_layout(&end.port.node_id);
            end.center = end_layout.port_center(&end.port);
        }

        // Determine bezier direction based on port kind
        let (start, end) = if drag.start_port.port.kind.is_source() {
            (drag.start_port.center, drag.current_pos)
        } else {
            (drag.current_pos, drag.start_port.center)
        };

        self.temp_connection_bezier
            .update_points(start, end, gui.scale());
        self.temp_connection_bezier.show(
            gui,
            Sense::hover(),
            "temp_connection",
            ConnectionBezierStyle::build(&gui.style, drag.start_port.port.kind, false, false),
        );
    }

    pub(crate) fn any_hovered(&self) -> bool {
        self.curves.iter().any(|c| c.hovered)
    }

    pub(crate) fn broke_iter(&self) -> impl Iterator<Item = &ConnectionKey> {
        self.curves
            .iter()
            .filter_map(|curve| curve.broke.then_some(&curve.key))
    }
}

// === Helpers ===

/// Advances an in-flight connection drag based on the pointer and the latest
/// port interaction command. The drag lives inside
/// [`crate::gui::interaction_state::Interaction::DraggingConnection`] — this
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
/// `handle_actions` — nothing here writes to the graph.
pub(crate) fn disconnect_connection(
    key: ConnectionKey,
    ctx: &GraphContext,
    ui_interaction: &mut GraphUiInteraction,
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
            ui_interaction.add_action(GraphUiAction::InputChanged {
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
            ui_interaction.add_action(GraphUiAction::EventConnectionChanged {
                event_node_id,
                event_idx,
                subscriber: trigger_node_id,
                change: EventSubscriberChange::Removed,
            });
        }
    }
}

fn apply_connection_deletions(
    deletions: Vec<ConnectionKey>,
    ctx: &GraphContext,
    ui_interaction: &mut GraphUiInteraction,
) {
    for key in deletions {
        disconnect_connection(key, ctx, ui_interaction);
    }
}

fn order_ports(port_a: PortRef, port_b: PortRef) -> (PortRef, PortRef) {
    match (port_a.kind, port_b.kind) {
        (PortKind::Output, PortKind::Input) => (port_b, port_a),
        (PortKind::Input, PortKind::Output) => (port_a, port_b),
        (PortKind::Event, PortKind::Trigger) => (port_b, port_a),
        (PortKind::Trigger, PortKind::Event) => (port_a, port_b),
        _ => unreachable!("ports must be of opposite types"),
    }
}

fn get_or_create_curve<'a>(
    compact: &'a mut CompactInsert<'_, ConnectionKey, ConnectionCurve>,
    key: ConnectionKey,
) -> &'a mut ConnectionCurve {
    compact.insert_with(&key, || ConnectionCurve::new(key)).1
}

/// Shared update-and-interact pass for one connection curve.
///
/// Used by both data (input-binding) and event (trigger) connections — they
/// differ only in which endpoints feed `start_pos` / `end_pos` and which
/// `port_kind` is used for styling.
#[allow(clippy::too_many_arguments)]
fn update_curve_interaction(
    gui: &mut Gui<'_>,
    curves: &mut CompactInsert<'_, ConnectionKey, ConnectionCurve>,
    deletions: &mut Vec<ConnectionKey>,
    key: ConnectionKey,
    start_pos: Pos2,
    end_pos: Pos2,
    port_kind: PortKind,
    breaker: Option<&ConnectionBreaker>,
) {
    let curve = get_or_create_curve(curves, key);
    curve.bezier.update_points(start_pos, end_pos, gui.scale());
    curve.broke = curve.bezier.intersects_breaker(breaker);

    let response = show_curve(gui, curve, port_kind);

    if breaker.is_some() {
        curve.hovered = false;
    } else {
        curve.hovered = response.hovered();
        if response.double_clicked_by(PointerButton::Primary) {
            deletions.push(key);
            curve.hovered = false;
        }
    }
}

fn render_highlight_curve(
    gui: &mut Gui<'_>,
    highlights: &mut CompactInsert<'_, ConnectionKey, ConnectionCurve>,
    key: ConnectionKey,
    start_pos: Pos2,
    end_pos: Pos2,
) {
    let curve = get_or_create_curve(highlights, key);
    curve.bezier.update_points(start_pos, end_pos, gui.scale());

    let style = ConnectionBezierStyle {
        start_color: gui.style.node.missing_inputs_shadow.color,
        end_color: gui.style.node.missing_inputs_shadow.color,
        stroke_width: gui.style.connections.stroke_width,
        feather: gui.style.connections.highlight_feather,
    };

    curve
        .bezier
        .show(gui, Sense::empty(), ("connection_highlight", key), style);
}

fn show_curve(
    gui: &mut Gui<'_>,
    curve: &mut ConnectionCurve,
    port_kind: PortKind,
) -> egui::Response {
    let style = ConnectionBezierStyle::build(&gui.style, port_kind, curve.broke, curve.hovered);
    curve.bezier.show(
        gui,
        Sense::click() | Sense::hover(),
        ("connection", curve.key),
        style,
    )
}

fn try_snap_to_port(drag: &mut ConnectionDrag, port_info: PortInfo) -> bool {
    drag.end_port = None;
    if drag.start_port.port.kind.opposite() == port_info.port.kind {
        drag.end_port = Some(port_info);
        drag.current_pos = port_info.center;
        true
    } else {
        false
    }
}

fn finish_drag(drag: &ConnectionDrag) -> ConnectionDragUpdate {
    if let Some(end_port) = drag.end_port {
        let (input_port, output_port) = order_ports(drag.start_port.port, end_port.port);
        ConnectionDragUpdate::FinishedWith {
            input_port,
            output_port,
        }
    } else {
        match drag.start_port.port.kind {
            PortKind::Input => ConnectionDragUpdate::FinishedWithEmptyOutput {
                input_port: drag.start_port.port,
            },
            PortKind::Output => ConnectionDragUpdate::FinishedWithEmptyInput {
                output_port: drag.start_port.port,
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
    use crate::gui::graph_layout::PortInfo;
    use crate::model::{ArgumentValuesCache, ViewGraph, ViewNode};
    use scenarium::function::FuncId;
    use scenarium::graph::{Event, Input, Node, NodeBehavior};
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
        drag.end_port = Some(port_info(NodeId::unique(), PortKind::Input, 0));

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
        assert_eq!(drag.end_port.unwrap().port, target.port);
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
        drag.end_port = Some(port_info(NodeId::unique(), PortKind::Input, 0));

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
        let mut cache = ArgumentValuesCache::default();
        let ctx = GraphContext {
            func_lib: &func_lib,
            view_graph: vg,
            execution_stats: None,
            autorun: false,
            argument_values_cache: &mut cache,
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

        let mut buf = GraphUiInteraction::default();
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

        let actions: Vec<_> = buf.action_stacks().flatten().cloned().collect();
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

        let mut buf = GraphUiInteraction::default();
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
        assert_eq!(buf.action_stacks().count(), 0);
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

        let mut buf = GraphUiInteraction::default();
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

        let actions: Vec<_> = buf.action_stacks().flatten().cloned().collect();
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

        let mut buf = GraphUiInteraction::default();
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
        assert_eq!(buf.action_stacks().count(), 0);
    }
}
