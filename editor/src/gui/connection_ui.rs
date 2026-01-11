use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use eframe::egui;
use egui::{PointerButton, Pos2, Sense};
use graph::graph::{NodeId, PortAddress};
use graph::prelude::{Binding, ExecutionStats};

use crate::common::UiEquals;
use crate::common::connection_bezier::{ConnectionBezier, ConnectionBezierStyle};
use crate::gui::Gui;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};

use crate::gui::graph_ui_interaction::{GraphUiAction, GraphUiInteraction};
use crate::gui::node_ui::PortDragInfo;

// todo merge with constBindUI
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ConnectionKey {
    pub(crate) input_node_id: NodeId,
    pub(crate) input_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PortKind {
    Input,
    Output,
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
            || !self.output_pos.ui_equals(&output_pos)
            || !self.input_pos.ui_equals(&input_pos);
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
    missing_curves: KeyIndexVec<ConnectionKey, ConnectionCurve>,

    pub(crate) temp_connection: Option<ConnectionDrag>,
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
        let mut compact = self.curves.compact_insert_start();
        let mut missing_compact = self.missing_curves.compact_insert_start();

        for node_view in &ctx.view_graph.view_nodes {
            let node_id = node_view.id;

            let inputs_len = ctx.view_graph.graph.by_id(&node_id).unwrap().inputs.len();

            for input_idx in 0..inputs_len {
                let (binding_target_id, binding_port_idx) = {
                    let node = ctx.view_graph.graph.by_id(&node_id).unwrap();
                    let input = &node.inputs[input_idx];
                    let Binding::Bind(binding) = &input.binding else {
                        continue;
                    };
                    (binding.target_id, binding.port_idx)
                };
                let connection_key = ConnectionKey {
                    input_node_id: node_id,
                    input_idx,
                };
                let output_layout = graph_layout.node_layout(&binding_target_id);
                let input_layout = graph_layout.node_layout(&node_id);

                let input_pos = input_layout.input_center(input_idx);
                let output_pos = output_layout.output_center(binding_port_idx);

                // =============

                let missing_inputs = execution_stats.is_some_and(|execution_stats| {
                    execution_stats.missing_inputs.contains(&PortAddress {
                        target_id: node_id,
                        port_idx: input_idx,
                    })
                });
                if missing_inputs {
                    let (_curve_idx, missing_curve) =
                        missing_compact.insert_with(&connection_key, || {
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
                        stroke_width: gui.style.connections.stroke_width * gui.scale,
                        feather: gui.style.node.missing_inputs_shadow.blur as f32,
                    };
                    missing_curve
                        .bezier
                        .update_points(output_pos, input_pos, gui.scale);
                    missing_curve.bezier.show(
                        gui,
                        Sense::empty(),
                        (
                            "connection_highlight",
                            connection_key.input_node_id,
                            connection_key.input_idx,
                        ),
                        style,
                    );
                }

                // =============

                let (_curve_idx, curve) =
                    compact.insert_with(&connection_key, || ConnectionCurve::new(connection_key));

                curve.bezier.update_points(output_pos, input_pos, gui.scale);
                curve.broke = curve.bezier.intersects_breaker(breaker);

                let style = gui
                    .style
                    .connections
                    .bezier_style(curve.broke, curve.hovered);

                let response = curve.bezier.show(
                    gui,
                    Sense::click() | Sense::hover(),
                    ("connection", curve.key.input_node_id, curve.key.input_idx),
                    style,
                );

                if breaker.is_some() {
                    curve.hovered = false;
                } else {
                    if response.double_clicked_by(PointerButton::Primary) {
                        let node = ctx
                            .view_graph
                            .graph
                            .by_id_mut(&curve.key.input_node_id)
                            .unwrap();
                        let input = &mut node.inputs[curve.key.input_idx];
                        let before = input.binding.clone();
                        input.binding = Binding::None;
                        let after = input.binding.clone();
                        ui_interaction.add_action(GraphUiAction::InputChanged {
                            node_id: curve.key.input_node_id,
                            input_idx: curve.key.input_idx,
                            before,
                            after,
                        });
                        curve.hovered = false;
                    }

                    curve.hovered = response.hovered();
                }
            }
        }

        if let Some(temp_connection) = &self.temp_connection {
            let (start, end) = match temp_connection.start_port.port.kind {
                PortKind::Input => (
                    temp_connection.current_pos,
                    temp_connection.start_port.center,
                ),
                PortKind::Output => (
                    temp_connection.start_port.center,
                    temp_connection.current_pos,
                ),
            };

            self.temp_connection_bezier
                .update_points(start, end, gui.scale);
            self.temp_connection_bezier.show(
                gui,
                Sense::hover(),
                "temp_connection",
                gui.style.connections.bezier_style(false, false),
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
            PortDragInfo::None => ConnectionDragUpdate::InProgress,
            PortDragInfo::DragStart(_) => unreachable!(),
            PortDragInfo::Hover(port_info) => {
                if drag.start_port.port.kind != port_info.port.kind {
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
            .filter_map(|curve| curve.broke.then_some(&curve.key))
    }
}

impl KeyIndexKey<ConnectionKey> for ConnectionCurve {
    fn key(&self) -> &ConnectionKey {
        &self.key
    }
}
