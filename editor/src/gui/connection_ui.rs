use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use eframe::egui;
use egui::{PointerButton, Pos2, Sense};
use graph::graph::NodeId;
use graph::prelude::Binding;

use crate::common::connection_bezier::ConnectionBezier;
use crate::gui::Gui;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::graph_ctx::GraphContext;
use crate::gui::graph_layout::{GraphLayout, PortInfo, PortRef};
use crate::gui::graph_ui::{GraphUiAction, GraphUiInteraction};
use crate::gui::node_ui::PortDragInfo;
use crate::model;

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
            || crate::common::pos_changed(self.output_pos, output_pos)
            || crate::common::pos_changed(self.input_pos, input_pos);
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

    pub(crate) temp_connection: Option<ConnectionDrag>,
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
        self.rebuild(gui, graph_layout, ctx.view_graph, breaker);

        for curve in self.curves.iter_mut() {
            let response = curve.bezier.show(
                gui,
                Sense::click() | Sense::hover(),
                ("connection", curve.key.input_node_id, curve.key.input_idx),
                curve.hovered,
                curve.broke,
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
                    node.inputs[curve.key.input_idx].binding = Binding::None;
                    ui_interaction.actions.push((
                        curve.key.input_node_id,
                        GraphUiAction::InputChanged {
                            input_idx: curve.key.input_idx,
                        },
                    ));
                    curve.hovered = false;
                }

                curve.hovered = response.hovered();
            }
        }
        if self.temp_connection.is_some() {
            self.temp_connection_bezier
                .show(gui, Sense::hover(), "temp_connection", false, false);
        }
    }

    pub(crate) fn start_drag(&mut self, port: PortInfo) {
        self.temp_connection = Some(ConnectionDrag::new(port));
    }

    pub(crate) fn update_drag(
        &mut self,
        pointer_pos: Pos2,
        drag_port_info: PortDragInfo,
    ) -> ConnectionDragUpdate {
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
                let update = drag
                    .end_port
                    .map_or(ConnectionDragUpdate::Finished, |end_port| {
                        let (input_port, output_port) =
                            match (drag.start_port.port.kind, end_port.port.kind) {
                                (PortKind::Output, PortKind::Input) => {
                                    (end_port.port, drag.start_port.port)
                                }
                                (PortKind::Input, PortKind::Output) => {
                                    (drag.start_port.port, end_port.port)
                                }
                                _ => unreachable!("ports must be of opposite types"),
                            };

                        ConnectionDragUpdate::FinishedWith {
                            input_port,
                            output_port,
                        }
                    });

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

    fn rebuild(
        &mut self,
        gui: &mut Gui<'_>,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
        breaker: Option<&ConnectionBreaker>,
    ) {
        let mut compact = self.curves.compact_insert_start();

        for node_view in &view_graph.view_nodes {
            let node = view_graph.graph.by_id(&node_view.id).unwrap();

            for (input_idx, input) in node.inputs.iter().enumerate() {
                let Binding::Bind(binding) = &input.binding else {
                    continue;
                };
                let connection_key = ConnectionKey {
                    input_node_id: node.id,
                    input_idx,
                };
                let (_curve_idx, curve) =
                    compact.insert_with(&connection_key, || ConnectionCurve::new(connection_key));

                let output_layout = graph_layout.node_layout(&binding.target_id);
                let input_layout = graph_layout.node_layout(&node.id);

                let input_pos = input_layout.input_center(input_idx);
                let output_pos = output_layout.output_center(binding.port_idx);

                curve.bezier.update_points(output_pos, input_pos, gui.scale);
                curve.broke = curve.bezier.intersects_breaker(breaker);
            }
        }

        if let Some(drag) = &mut self.temp_connection {
            let (start, end) = match drag.start_port.port.kind {
                PortKind::Input => (drag.current_pos, drag.start_port.center),
                PortKind::Output => (drag.start_port.center, drag.current_pos),
            };

            self.temp_connection_bezier
                .update_points(start, end, gui.scale);
        }
    }
}

impl KeyIndexKey<ConnectionKey> for ConnectionCurve {
    fn key(&self) -> &ConnectionKey {
        &self.key
    }
}
