use glam::Vec2;
use palantir::{Background, Color, Configure, LineCap, LineJoin, Panel, Shape, Sizing, Ui};
use scenarium::prelude::NodeId;
use std::collections::HashMap;

use crate::frame_result::FrameResult;
use crate::gui::node_ui::{NodePortSpans, NodeUI};
use crate::scene::Scene;

const CONN_WIDTH: f32 = 2.0;
const CANVAS_BG: u32 = 0x1e1e1e;
const CONN_COLOR: u32 = 0x9ec1ff;

/// Port centers captured at the end of frame N and consumed at the
/// start of frame N+1. Lets connections draw *before* nodes (so
/// beziers land behind node bodies) while still threading the real
/// laid-out port centers. Same one-frame lag the prior `Response::rect`
/// snapshot had, just hoisted into an explicit interframe carrier.
///
/// Flat layout: `centers` pools all `Vec2`s in node-then-input-then-output
/// order; `nodes` maps a `NodeId` to the pair of `PortSpan`s slicing
/// into the pool. A node only earns an entry once every one of its
/// ports resolved a layout rect (frame 2+); first-frame nodes are
/// absent from `nodes`, so `draw_connections` skips them via
/// `nodes.get(&id)`.
#[derive(Default, Debug)]
pub struct PortCache {
    pub centers: Vec<Vec2>,
    pub nodes: HashMap<NodeId, NodePortSpans>,
}

impl PortCache {
    pub fn clear(&mut self) {
        self.centers.clear();
        self.nodes.clear();
    }
}

/// Canvas-level UI scope: owns the port cache (interframe port-center
/// snapshot used to anchor connection beziers) and the `NodeUI` that
/// renders every graph node. `frame` draws connections from the
/// previous frame's snapshot, then delegates node rendering to
/// `NodeUI::draw_all` which refills the cache.
#[derive(Default, Debug)]
pub struct GraphUI {
    pub ports: PortCache,
    pub node_ui: NodeUI,
}

impl GraphUI {
    /// Pre-record pass — see
    /// [`crate::gui::node_ui::NodeUI::prepass`].
    pub fn prepass(&self, ui: &Ui, out: &mut FrameResult) {
        self.node_ui.prepass(ui, out);
    }

    pub fn frame(&mut self, ui: &mut Ui, scene: &Scene, out: &mut FrameResult) {
        let Self { ports, node_ui } = self;
        Panel::canvas()
            .id_salt("graph.canvas")
            .size((Sizing::FILL, Sizing::FILL))
            .background(Background {
                fill: Color::hex(CANVAS_BG).into(),
                ..Default::default()
            })
            .show(ui, |ui| {
                draw_connections(ui, scene, ports);
                ports.clear();
                node_ui.draw_all(ui, scene, ports, out);
            });
    }
}

fn draw_connections(ui: &mut Ui, scene: &Scene, ports: &PortCache) {
    let color = Color::hex(CONN_COLOR);
    for c in &scene.connections {
        let (Some(src), Some(tgt)) = (ports.nodes.get(&c.src_node), ports.nodes.get(&c.tgt_node))
        else {
            continue;
        };
        let (Some(p0), Some(p3)) = (
            src.outputs.get(&ports.centers, c.src_port),
            tgt.inputs.get(&ports.centers, c.tgt_port),
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
