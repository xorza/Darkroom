use glam::Vec2;
use palantir::{Background, Color, Configure, LineCap, LineJoin, Panel, Shape, Sizing, Ui};
use scenarium::prelude::NodeId;
use std::collections::HashMap;

use crate::gui::node_widget::{self, NodePorts};
use crate::scene::Scene;

const CONN_WIDTH: f32 = 2.0;
const CANVAS_BG: u32 = 0x1e1e1e;
const CONN_COLOR: u32 = 0x9ec1ff;

pub fn build(ui: &mut Ui, scene: &Scene) {
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
            let port_rects = draw_nodes(ui, scene);
            draw_connections(ui, scene, &port_rects);
        });
}

fn draw_nodes(ui: &mut Ui, scene: &Scene) -> HashMap<NodeId, NodePorts> {
    let mut map = HashMap::with_capacity(scene.nodes.len());
    for n in &scene.nodes {
        let ports = node_widget::draw(ui, scene, n);
        map.insert(n.id, ports);
    }
    map
}

fn draw_connections(ui: &mut Ui, scene: &Scene, ports: &HashMap<NodeId, NodePorts>) {
    let color = Color::hex(CONN_COLOR);
    for c in &scene.connections {
        let (Some(src_ports), Some(tgt_ports)) = (ports.get(&c.src_node), ports.get(&c.tgt_node))
        else {
            continue;
        };
        let (Some(p0), Some(p3)) = (
            src_ports.outputs.get(c.src_port).copied().flatten(),
            tgt_ports.inputs.get(c.tgt_port).copied().flatten(),
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
