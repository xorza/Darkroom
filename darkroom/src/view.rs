use glam::Vec2;
use palantir::{
    Background, Color, Configure, Corners, LineCap, LineJoin, Panel, Shape, Sizing, Stroke, Text,
    Ui,
};
use scenarium::prelude::{Binding, NodeId};
use std::collections::HashMap;

use crate::model::ViewGraph;

const NODE_W: f32 = 160.0;
const NODE_H: f32 = 70.0;
const CONN_WIDTH: f32 = 2.0;

pub fn build(ui: &mut Ui<()>, view_graph: &ViewGraph) {
    Panel::canvas()
        .id_salt("graph.canvas")
        .size((Sizing::FILL, Sizing::FILL))
        .background(Background {
            fill: Color::hex(0x1e1e1e).into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            let positions: HashMap<NodeId, Vec2> = view_graph
                .view_nodes
                .iter()
                .map(|vn| (vn.id, vn.pos))
                .collect();
            draw_connections(ui, view_graph, &positions);
            draw_nodes(ui, view_graph);
        });
}

fn draw_connections(ui: &mut Ui<()>, view_graph: &ViewGraph, positions: &HashMap<NodeId, Vec2>) {
    let color = Color::hex(0x9ec1ff);
    for node in view_graph.graph.iter() {
        let Some(&tgt_pos) = positions.get(&node.id) else {
            continue;
        };
        let p3 = tgt_pos + Vec2::new(0.0, NODE_H * 0.5);
        for inp in &node.inputs {
            let Binding::Bind(addr) = &inp.binding else {
                continue;
            };
            let Some(&src_pos) = positions.get(&addr.target_id) else {
                continue;
            };
            let p0 = src_pos + Vec2::new(NODE_W, NODE_H * 0.5);
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

fn draw_nodes(ui: &mut Ui<()>, view_graph: &ViewGraph) {
    let bg = Background {
        fill: Color::hex(0x2d2d33).into(),
        stroke: Stroke::solid(Color::hex(0x5a5a66), 1.0),
        radius: Corners::all(6.0),
        ..Default::default()
    };
    for vn in view_graph.view_nodes.iter() {
        let Some(node) = view_graph.graph.by_id(&vn.id) else {
            continue;
        };
        Panel::vstack()
            .id_salt(("graph.node", vn.id.as_u128()))
            .position(vn.pos)
            .size((Sizing::Fixed(NODE_W), Sizing::Fixed(NODE_H)))
            .padding(10.0)
            .background(bg)
            .show(ui, |ui| {
                Text::new(node.name.clone()).show(ui);
            });
    }
}
