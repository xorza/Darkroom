use glam::Vec2;
use palantir::{Background, Color, Configure, LineCap, LineJoin, Panel, Shape, Sizing, Ui};
use scenarium::prelude::{Binding, NodeId};
use std::collections::HashMap;

use crate::AppState;
use crate::gui::node_widget::{self, NodePorts};

const CONN_WIDTH: f32 = 2.0;
const CANVAS_BG: u32 = 0x1e1e1e;
const CONN_COLOR: u32 = 0x9ec1ff;

pub fn build(ui: &mut Ui, app: &AppState) {
    Panel::canvas()
        .id_salt("graph.canvas")
        .size((Sizing::FILL, Sizing::FILL))
        .background(Background {
            fill: Color::hex(CANVAS_BG).into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            // Nodes first so their port `Frame`s emit responses whose
            // `rect()` (prior-frame layout snapshot) gives us exact
            // endpoint positions. Connections drawn second land
            // visually on top of the node bodies — acceptable for now;
            // moving them to a layer behind nodes is a later cleanup.
            let port_rects = draw_nodes(ui, app);
            draw_connections(ui, app, &port_rects);
        });
}

fn draw_nodes(ui: &mut Ui, app: &AppState) -> HashMap<NodeId, NodePorts> {
    let mut map = HashMap::with_capacity(app.view_graph.view_nodes.len());
    for vn in app.view_graph.view_nodes.iter() {
        let Some(node) = app.view_graph.graph.by_id(&vn.id) else {
            continue;
        };
        let Some(func) = app.func_lib.by_id(&node.func_id) else {
            continue;
        };
        let ports = node_widget::draw(ui, vn, node, func);
        map.insert(vn.id, ports);
    }
    map
}

fn draw_connections(ui: &mut Ui, app: &AppState, ports: &HashMap<NodeId, NodePorts>) {
    let color = Color::hex(CONN_COLOR);
    for node in app.view_graph.graph.iter() {
        let Some(tgt_ports) = ports.get(&node.id) else {
            continue;
        };
        for (i, inp) in node.inputs.iter().enumerate() {
            let Binding::Bind(addr) = &inp.binding else {
                continue;
            };
            let Some(src_ports) = ports.get(&addr.target_id) else {
                continue;
            };
            let (Some(p0), Some(p3)) = (
                src_ports.outputs.get(addr.port_idx).copied().flatten(),
                tgt_ports.inputs.get(i).copied().flatten(),
            ) else {
                continue;
            };
            let dx = ((p3.x - p0.x).abs() * 0.5).max(40.0);
            ui.add_shape(Shape::CubicBezier {
                p0,
                p1: p0 + Vec2::new(dx, 0.0),
                p2: p3 - Vec2::new(dx, 0.0),
                p3,
                width: CONN_WIDTH,
                brush: color.into(),
                cap: LineCap::Round,
                join: LineJoin::Miter,
                tolerance: 0.5,
            });
        }
    }
}
