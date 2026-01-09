use common::key_index_vec::{KeyIndexKey, KeyIndexVec};
use eframe::egui;
use egui::{PointerButton, Pos2, Sense};
use graph::graph::NodeId;
use graph::prelude::Binding;
use std::collections::HashSet;

use crate::common::bezier::Bezier;
use crate::gui::Gui;
use crate::gui::connection_breaker::ConnectionBreaker;
use crate::gui::graph_layout::{GraphLayout, PortInfo};
use crate::gui::node_ui::PortDragInfo;
use crate::model;

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
    endpoints: ConnectionEndpoints,
}

#[derive(Debug, Clone)]
pub(crate) enum ConnectionDragUpdate {
    InProgress,
    Finished,
    FinishedWith {
        start_port: PortInfo,
        end_port: PortInfo,
    },
}

impl ConnectionDrag {
    pub(crate) fn new(port: PortInfo) -> Self {
        Self {
            current_pos: port.center,
            start_port: port,
            end_port: None,
            endpoints: ConnectionEndpoints::default(),
        }
    }
}

#[derive(Debug, Clone)]
struct ConnectionCurve {
    key: ConnectionKey,
    highlighted: bool,
    endpoints: ConnectionEndpoints,
    bezier: Bezier,
}

impl ConnectionCurve {
    fn new(key: ConnectionKey) -> Self {
        Self {
            key,
            highlighted: false,
            endpoints: ConnectionEndpoints::default(),
            bezier: Bezier::new(),
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

#[derive(Debug)]
pub(crate) struct ConnectionUi {
    curves: KeyIndexVec<ConnectionKey, ConnectionCurve>,
    pub(crate) broke: HashSet<ConnectionKey>,

    hovered: HashSet<ConnectionKey>,

    temp_connection: Option<ConnectionDrag>,
    temp_connection_bezier: Bezier,
}

impl Default for ConnectionUi {
    fn default() -> Self {
        Self {
            curves: KeyIndexVec::default(),
            broke: HashSet::default(),
            hovered: HashSet::default(),
            temp_connection: None,
            temp_connection_bezier: Bezier::new(),
        }
    }
}

impl ConnectionUi {
    pub(crate) fn render(
        &mut self,
        gui: &mut Gui<'_>,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
        breaker: Option<&ConnectionBreaker>,
    ) {
        self.rebuild(gui, graph_layout, view_graph, breaker);

        for curve in &self.curves {
            let response = curve.bezier.show(
                gui,
                Sense::click() | Sense::hover(),
                ("connection", curve.key.input_node_id, curve.key.input_idx),
            );
            if response.double_clicked_by(PointerButton::Primary) {
                todo!()
                // self.broke.insert(curve.key);
            }
            if response.hovered() {
                self.hovered.insert(curve.key);
            }
        }
        if self.temp_connection.is_some() {
            self.temp_connection_bezier
                .show(gui, Sense::hover(), "temp_connection");
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
                        ConnectionDragUpdate::FinishedWith {
                            start_port: drag.start_port,
                            end_port,
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

    fn rebuild(
        &mut self,
        gui: &mut Gui<'_>,
        graph_layout: &GraphLayout,
        view_graph: &model::ViewGraph,
        breaker: Option<&ConnectionBreaker>,
    ) {
        self.broke.clear();

        let mut write_idx: usize = 0;

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
                let curve_idx =
                    self.curves
                        .compact_insert_with(&connection_key, &mut write_idx, || {
                            ConnectionCurve::new(connection_key)
                        });
                let curve = &mut self.curves[curve_idx];

                let output_layout = graph_layout.node_layout(&binding.target_id);
                let input_layout = graph_layout.node_layout(&node.id);

                let input_pos = input_layout.input_center(input_idx);
                let output_pos = output_layout.output_center(binding.port_idx);

                let needs_rebuild = curve.endpoints.update(output_pos, input_pos);
                if needs_rebuild {
                    curve.bezier.build_points(output_pos, input_pos, gui.scale);
                }

                let highlighted = if let Some(segments) = breaker.map(|breaker| breaker.segments())
                {
                    let mut hit = false;
                    'outer: for (b1, b2) in segments {
                        let curve_segments = curve
                            .bezier
                            .points()
                            .windows(2)
                            .map(|pair| (pair[0], pair[1]));

                        for (a1, a2) in curve_segments {
                            if ConnectionBezier::segments_intersect(a1, a2, b1, b2) {
                                self.broke.insert(connection_key);
                                hit = true;
                                break 'outer;
                            }
                        }
                    }

                    hit
                } else {
                    false
                };

                if curve.highlighted != highlighted || needs_rebuild {
                    curve.highlighted = highlighted;

                    if curve.highlighted {
                        curve.bezier.build_mesh(
                            gui.style.connections.highlight_stroke.color,
                            gui.style.connections.highlight_stroke.color,
                            gui.style.connections.highlight_stroke.width,
                        );
                    } else {
                        curve.bezier.build_mesh(
                            gui.style.node.output_port_color,
                            gui.style.node.input_port_color,
                            gui.style.connections.stroke_width,
                        );
                    };
                }
            }
        }

        self.curves.compact_finish(write_idx);

        if let Some(drag) = &mut self.temp_connection {
            let (start, end) = match drag.start_port.port.kind {
                PortKind::Input => (drag.current_pos, drag.start_port.center),
                PortKind::Output => (drag.start_port.center, drag.current_pos),
            };
            let needs_rebuild = drag.endpoints.update(start, end);
            if needs_rebuild {
                self.temp_connection_bezier
                    .build_points(start, end, gui.scale);
                self.temp_connection_bezier.build_mesh(
                    gui.style.node.output_port_color,
                    gui.style.node.input_port_color,
                    gui.style.connections.stroke_width,
                );
            }
        }
    }
}

impl KeyIndexKey<ConnectionKey> for ConnectionCurve {
    fn key(&self) -> &ConnectionKey {
        &self.key
    }
}
use crate::common::connection_bezier::ConnectionBezier;
