use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use eframe::egui;
use egui::{PointerButton, Pos2, Sense};
use graph::graph::{NodeId, PortAddress};
use graph::prelude::{Binding, ExecutionStats};
use graph::worker::EventRef;

use crate::common::UiEquals;
use crate::common::connection_bezier::{ConnectionBezier, ConnectionBezierStyle};
use crate::gui::Gui;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};

use crate::gui::graph_ui_interaction::GraphUiInteraction;
use crate::gui::node_ui::PortDragInfo;
use crate::model::EventSubscriberChange;
use crate::model::graph_ui_action::GraphUiAction;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PortKind {
    Input,
    Output,

    Trigger,
    Event,
}

#[derive(Debug)]
pub(crate) struct ConnectionDrag {
    pub(crate) start_port: PortInfo,
    pub(crate) end_port: Option<PortInfo>,
    pub(crate) current_pos: Pos2,
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, Copy)]
struct ConnectionEndpoints {
    inited: bool,
    output_pos: Pos2,
    input_pos: Pos2,
}

impl Default for ConnectionEndpoints {
    fn default() -> Self {
        Self {
            inited: false,
            output_pos: Pos2::ZERO,
            input_pos: Pos2::ZERO,
        }
    }
}

impl ConnectionEndpoints {
    fn update(&mut self, output_pos: Pos2, input_pos: Pos2) -> bool {
        let needs_rebuild = !self.inited
            || !self.output_pos.ui_equals(output_pos)
            || !self.input_pos.ui_equals(input_pos);
        if needs_rebuild {
            self.inited = true;
            self.output_pos = output_pos;
            self.input_pos = input_pos;
        }
        needs_rebuild
    }
}

#[derive(Debug, Default)]
pub(crate) struct ConnectionUi {
    curves: KeyIndexVec<ConnectionKey, ConnectionCurve>,
    missing_data_curves: KeyIndexVec<ConnectionKey, ConnectionCurve>,
    highlighted_event_curves: KeyIndexVec<ConnectionKey, ConnectionCurve>,

    temp_connection: Option<ConnectionDrag>,
    temp_connection_bezier: ConnectionBezier,
}

impl ConnectionUi {
    pub(crate) fn render(
        &mut self,
        gui: &mut Gui<'_>,
        ctx: &mut GraphContext,
        graph_layout: &GraphLayout,
        execution_stats: Option<&ExecutionStats>,
        ui_interaction: &mut GraphUiInteraction,
        breaker: Option<&ConnectionBreaker>,
    ) {
        let mut curves_compact = self.curves.compact_insert_start();
        let mut missing_data_curves_compact = self.missing_data_curves.compact_insert_start();
        let mut highlighted_event_curves_compact =
            self.highlighted_event_curves.compact_insert_start();

        for node_view in &ctx.view_graph.view_nodes {
            let node_id = node_view.id;
            let node = ctx.view_graph.graph.by_id_mut(&node_id).unwrap();

            let input_layout = graph_layout.node_layout(&node_id);

            for (input_idx, input) in node.inputs.iter_mut().enumerate() {
                let Binding::Bind(binding) = &input.binding else {
                    continue;
                };

                let connection_key = ConnectionKey::Input {
                    input_node_id: node_id,
                    input_idx,
                };
                let output_layout = graph_layout.node_layout(&binding.target_id);

                let input_pos = input_layout.input_center(input_idx);
                let output_pos = output_layout.output_center(binding.port_idx);

                // =============

                let input_missing = execution_stats.is_some_and(|execution_stats| {
                    execution_stats.missing_inputs.contains(&PortAddress {
                        target_id: node_id,
                        port_idx: input_idx,
                    })
                });
                if input_missing {
                    let (_curve_idx, missing_curve) =
                        missing_data_curves_compact.insert_with(&connection_key, || {
                            let bezier = ConnectionBezier::default();

                            ConnectionCurve {
                                key: connection_key,
                                broke: false,
                                hovered: false,
                                bezier,
                            }
                        });

                    let style = ConnectionBezierStyle {
                        start_color: gui.style.node.missing_inputs_shadow.color,
                        end_color: gui.style.node.missing_inputs_shadow.color,
                        stroke_width: gui.style.connections.stroke_width,
                        feather: gui.style.connections.highlight_feather,
                    };
                    missing_curve
                        .bezier
                        .update_points(output_pos, input_pos, gui.scale());
                    missing_curve.bezier.show(
                        gui,
                        Sense::empty(),
                        ("connection_highlight", connection_key),
                        style,
                    );
                }

                // =============

                let (_curve_idx, data_curve) = curves_compact
                    .insert_with(&connection_key, || ConnectionCurve::new(connection_key));

                data_curve
                    .bezier
                    .update_points(output_pos, input_pos, gui.scale());
                data_curve.broke = data_curve.bezier.intersects_breaker(breaker);

                let style = ConnectionBezierStyle::build(
                    &gui.style,
                    PortKind::Input,
                    data_curve.broke,
                    data_curve.hovered,
                );

                let response = data_curve.bezier.show(
                    gui,
                    Sense::click() | Sense::hover(),
                    ("connection", data_curve.key),
                    style,
                );

                if breaker.is_some() {
                    data_curve.hovered = false;
                } else {
                    if response.double_clicked_by(PointerButton::Primary) {
                        let before = input.binding.clone();
                        input.binding = Binding::None;
                        let after = input.binding.clone();
                        ui_interaction.add_action(GraphUiAction::InputChanged {
                            node_id,
                            input_idx,
                            before,
                            after,
                        });
                        data_curve.hovered = false;
                    }

                    data_curve.hovered = response.hovered();
                }
            }

            for (event_idx, event) in node.events.iter_mut().enumerate() {
                let event_layout = graph_layout.node_layout(&node_id);
                let event_pos = event_layout.event_center(event_idx);

                for subscriber_idx in (0..event.subscribers.len()).rev() {
                    let trigger_node_id = event.subscribers[subscriber_idx];
                    let trigger_layout = graph_layout.node_layout(&trigger_node_id);
                    let trigger_pos = trigger_layout.trigger_center();

                    let connection_key = ConnectionKey::Event {
                        event_node_id: node_id,
                        event_idx,
                        trigger_node_id,
                    };

                    let highlighted = execution_stats.is_some_and(|exe_stats| {
                        exe_stats
                            .triggered_events
                            .contains(&EventRef { node_id, event_idx })
                    });
                    if highlighted {
                        let (_idx, highlighted) =
                            highlighted_event_curves_compact.insert_with(&connection_key, || {
                                let bezier = ConnectionBezier::default();

                                ConnectionCurve {
                                    key: connection_key,
                                    broke: false,
                                    hovered: false,
                                    bezier,
                                }
                            });
                        let style = ConnectionBezierStyle {
                            start_color: gui.style.node.missing_inputs_shadow.color,
                            end_color: gui.style.node.missing_inputs_shadow.color,
                            stroke_width: gui.style.connections.stroke_width,
                            feather: gui.style.connections.highlight_feather,
                        };
                        highlighted
                            .bezier
                            .update_points(event_pos, trigger_pos, gui.scale());
                        highlighted.bezier.show(
                            gui,
                            Sense::empty(),
                            ("connection_highlight", connection_key),
                            style,
                        );
                    }

                    let (_curve_idx, event_curve) = curves_compact
                        .insert_with(&connection_key, || ConnectionCurve::new(connection_key));

                    event_curve
                        .bezier
                        .update_points(event_pos, trigger_pos, gui.scale());
                    event_curve.broke = event_curve.bezier.intersects_breaker(breaker);

                    let style = ConnectionBezierStyle::build(
                        &gui.style,
                        PortKind::Trigger,
                        event_curve.broke,
                        event_curve.hovered,
                    );

                    let response = event_curve.bezier.show(
                        gui,
                        Sense::click() | Sense::hover(),
                        ("connection", event_curve.key),
                        style,
                    );

                    if breaker.is_some() {
                        event_curve.hovered = false;
                    } else {
                        event_curve.hovered = response.hovered();

                        if response.double_clicked_by(PointerButton::Primary) {
                            let subscriber = event.subscribers[subscriber_idx];
                            event.subscribers.remove(subscriber_idx);

                            ui_interaction.add_action(GraphUiAction::EventConnectionChanged {
                                event_node_id: node_id,
                                event_idx,
                                subscriber,
                                change: EventSubscriberChange::Removed,
                            });
                            event_curve.hovered = false;
                        }
                    }
                }
            }
        }

        if let Some(temp_connection) = &self.temp_connection {
            let (start, end) = match temp_connection.start_port.port.kind {
                PortKind::Trigger | PortKind::Input => (
                    temp_connection.current_pos,
                    temp_connection.start_port.center,
                ),
                PortKind::Event | PortKind::Output => (
                    temp_connection.start_port.center,
                    temp_connection.current_pos,
                ),
            };

            self.temp_connection_bezier
                .update_points(start, end, gui.scale());
            self.temp_connection_bezier.show(
                gui,
                Sense::hover(),
                "temp_connection",
                ConnectionBezierStyle::build(
                    &gui.style,
                    temp_connection.start_port.port.kind,
                    false,
                    false,
                ),
            );
        }
    }

    pub(crate) fn update_drag(
        &mut self,
        pointer_pos: Pos2,
        drag_port_info: PortDragInfo,
    ) -> ConnectionDragUpdate {
        if let PortDragInfo::DragStart(port_info) = drag_port_info {
            self.temp_connection = Some(ConnectionDrag::new(port_info));
            return ConnectionDragUpdate::InProgress;
        }

        if self.temp_connection.is_none() {
            return ConnectionDragUpdate::Finished;
        }

        let drag = self.temp_connection.as_mut().unwrap();
        drag.current_pos = pointer_pos;

        match drag_port_info {
            PortDragInfo::None => {
                drag.end_port = None;
                ConnectionDragUpdate::InProgress
            }
            PortDragInfo::DragStart(_) => unreachable!(),
            PortDragInfo::Hover(port_info) => {
                drag.end_port = None;
                if drag.start_port.port.kind.opposite() == port_info.port.kind {
                    drag.end_port = Some(port_info);
                    drag.current_pos = port_info.center;
                }

                ConnectionDragUpdate::InProgress
            }
            PortDragInfo::DragStop => {
                let update = if let Some(port_info) = drag.end_port {
                    let (input_port, output_port) =
                        match (drag.start_port.port.kind, port_info.port.kind) {
                            (PortKind::Output, PortKind::Input) => {
                                (port_info.port, drag.start_port.port)
                            }
                            (PortKind::Input, PortKind::Output) => {
                                (drag.start_port.port, port_info.port)
                            }
                            (PortKind::Event, PortKind::Trigger) => {
                                (port_info.port, drag.start_port.port)
                            }
                            (PortKind::Trigger, PortKind::Event) => {
                                (drag.start_port.port, port_info.port)
                            }
                            _ => unreachable!("ports must be of opposite types"),
                        };

                    ConnectionDragUpdate::FinishedWith {
                        input_port,
                        output_port,
                    }
                } else if drag.start_port.port.kind == PortKind::Input {
                    ConnectionDragUpdate::FinishedWithEmptyOutput {
                        input_port: drag.start_port.port,
                    }
                } else if drag.start_port.port.kind == PortKind::Output {
                    ConnectionDragUpdate::FinishedWithEmptyInput {
                        output_port: drag.start_port.port,
                    }
                } else {
                    ConnectionDragUpdate::Finished
                };

                self.stop_drag();

                update
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
            .filter_map(|data_curve| data_curve.broke.then_some(&data_curve.key))
    }
}

impl KeyIndexKey<ConnectionKey> for ConnectionCurve {
    fn key(&self) -> &ConnectionKey {
        &self.key
    }
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
}
