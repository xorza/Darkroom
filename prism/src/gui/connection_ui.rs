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

    pub temp_connection: Option<ConnectionDrag>,
    temp_connection_bezier: ConnectionBezier,
}

impl ConnectionUi {
    pub(crate) fn render(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext,
        graph_layout: &GraphLayout,
        ui_interaction: &mut GraphUiInteraction,
        breaker: Option<&ConnectionBreaker>,
    ) {
        let execution_stats = ctx.execution_stats;
        let mut curves = self.curves.compact_insert_start();
        let mut highlights = self.highlight_curves.compact_insert_start();

        for node_view in &ctx.view_graph.view_nodes {
            let node_id = node_view.id;
            let node = ctx.view_graph.graph.by_id_mut(&node_id).unwrap();
            let node_layout = graph_layout.node_layout(&node_id);

            // Render data connections
            for (input_idx, input) in node.inputs.iter_mut().enumerate() {
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

                let curve = get_or_create_curve(&mut curves, key);
                curve
                    .bezier
                    .update_points(output_pos, input_pos, gui.scale());
                curve.broke = curve.bezier.intersects_breaker(breaker);

                let response = show_curve(gui, curve, PortKind::Input);

                if breaker.is_some() {
                    curve.hovered = false;
                } else {
                    curve.hovered = response.hovered();

                    if response.double_clicked_by(PointerButton::Primary) {
                        let before = input.binding.clone();
                        input.binding = Binding::None;
                        ui_interaction.add_action(GraphUiAction::InputChanged {
                            node_id,
                            input_idx,
                            before,
                            after: Binding::None,
                        });
                        curve.hovered = false;
                    }
                }
            }

            // Render event connections
            for (event_idx, event) in node.events.iter_mut().enumerate() {
                let event_pos = node_layout.event_center(event_idx);

                for subscriber_idx in (0..event.subscribers.len()).rev() {
                    let trigger_node_id = event.subscribers[subscriber_idx];
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

                    let curve = get_or_create_curve(&mut curves, key);
                    curve
                        .bezier
                        .update_points(event_pos, trigger_pos, gui.scale());
                    curve.broke = curve.bezier.intersects_breaker(breaker);

                    let response = show_curve(gui, curve, PortKind::Trigger);

                    if breaker.is_some() {
                        curve.hovered = false;
                    } else {
                        curve.hovered = response.hovered();

                        if response.double_clicked_by(PointerButton::Primary) {
                            let subscriber = event.subscribers.remove(subscriber_idx);
                            ui_interaction.add_action(GraphUiAction::EventConnectionChanged {
                                event_node_id: node_id,
                                event_idx,
                                subscriber,
                                change: EventSubscriberChange::Removed,
                            });
                            curve.hovered = false;
                        }
                    }
                }
            }
        }

        drop(curves);
        drop(highlights);

        self.render_temp_connection(gui, graph_layout);
    }

    fn render_temp_connection(&mut self, gui: &mut Gui<'_>, graph_layout: &GraphLayout) {
        let Some(drag) = &mut self.temp_connection else {
            return;
        };

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

    pub(crate) fn update_drag(
        &mut self,
        pointer_pos: Pos2,
        cmd: PortInteractCommand,
    ) -> ConnectionDragUpdate {
        if let PortInteractCommand::DragStart(port_info) = cmd {
            self.temp_connection = Some(ConnectionDrag::new(port_info));
            return ConnectionDragUpdate::InProgress;
        }

        let drag = self.temp_connection.as_mut().expect("missing DragStart");
        drag.current_pos = pointer_pos;

        match cmd {
            PortInteractCommand::None => {
                drag.end_port = None;
                ConnectionDragUpdate::InProgress
            }
            PortInteractCommand::DragStart(_) => unreachable!(),
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

    pub(crate) fn stop_drag(&mut self) {
        self.temp_connection = None;
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
