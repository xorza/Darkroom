use glam::Vec2;
use palantir::{Background, Color, Configure, LineCap, LineJoin, Panel, Shape, Sizing, Ui};
use scenarium::prelude::{Binding, NodeId};
use std::collections::HashMap;

use crate::AppState;
use crate::gui::node_widget;
use crate::gui::{HEADER_H, NODE_W, PORT_COL_PAD_TOP, PORT_GAP, PORT_RADIUS, PORT_SIZE, Side};

const CONN_WIDTH: f32 = 2.0;
const CANVAS_BG: u32 = 0x1e1e1e;
const CONN_COLOR: u32 = 0x9ec1ff;

/// Per-node world-space port centers. Indexed positionally — matches
/// `Node.inputs[i]` / `Func.outputs[i]`. Computed from the same
/// constants the widget tree uses, so the rendered port circles line
/// up with these endpoints.
struct NodePorts {
    inputs: Vec<Vec2>,
    outputs: Vec<Vec2>,
}

pub fn build(ui: &mut Ui, app: &AppState) {
    Panel::canvas()
        .id_salt("graph.canvas")
        .size((Sizing::FILL, Sizing::FILL))
        .background(Background {
            fill: Color::hex(CANVAS_BG).into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            let ports = compute_ports(app);
            draw_connections(ui, app, &ports);
            draw_nodes(ui, app);
        });
}

fn compute_ports(app: &AppState) -> HashMap<NodeId, NodePorts> {
    app.view_graph
        .view_nodes
        .iter()
        .filter_map(|vn| {
            let node = app.view_graph.graph.by_id(&vn.id)?;
            let func = app.func_lib.by_id(&node.func_id)?;
            let inputs = port_centers(vn.pos, Side::Left, node.inputs.len());
            let outputs = port_centers(vn.pos, Side::Right, func.outputs.len());
            Some((vn.id, NodePorts { inputs, outputs }))
        })
        .collect()
}

/// Stacks `n` port centers along a node edge. Mirrors the widget-tree
/// math so the connection endpoints land on the rendered circles.
fn port_centers(node_pos: Vec2, side: Side, n: usize) -> Vec<Vec2> {
    let edge_x = match side {
        Side::Left => node_pos.x,
        Side::Right => node_pos.x + NODE_W,
    };
    (0..n)
        .map(|i| {
            let y = node_pos.y
                + HEADER_H
                + PORT_COL_PAD_TOP
                + (i as f32) * (PORT_SIZE + PORT_GAP)
                + PORT_RADIUS;
            Vec2::new(edge_x, y)
        })
        .collect()
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
            let (Some(&p0), Some(&p3)) =
                (src_ports.outputs.get(addr.port_idx), tgt_ports.inputs.get(i))
            else {
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

fn draw_nodes(ui: &mut Ui, app: &AppState) {
    for vn in app.view_graph.view_nodes.iter() {
        let Some(node) = app.view_graph.graph.by_id(&vn.id) else {
            continue;
        };
        let Some(func) = app.func_lib.by_id(&node.func_id) else {
            continue;
        };
        node_widget::draw(ui, vn, node, func);
    }
}
