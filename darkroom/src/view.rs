use glam::Vec2;
use palantir::{Background, Color, Configure, LineCap, LineJoin, Panel, Shape, Sizing, Ui};

use crate::frame_cache::{DragAnchor, FrameCache, PortCache};
use crate::frame_result::FrameResult;
use crate::gui::node_widget;
use crate::intent::Intent;
use crate::scene::Scene;

const CONN_WIDTH: f32 = 2.0;
const CANVAS_BG: u32 = 0x1e1e1e;
const CONN_COLOR: u32 = 0x9ec1ff;

pub fn build(ui: &mut Ui, scene: &Scene, cache: &mut FrameCache, out: &mut FrameResult) {
    Panel::canvas()
        .id_salt("graph.canvas")
        .size((Sizing::FILL, Sizing::FILL))
        .background(Background {
            fill: Color::hex(CANVAS_BG).into(),
            ..Default::default()
        })
        .show(ui, |ui| {
            draw_connections(ui, scene, &cache.ports);
            cache.clear();
            for n in &scene.nodes {
                let result = node_widget::draw(ui, scene, n, &mut cache.ports.centers);
                if let Some(spans) = result.spans {
                    cache.ports.nodes.insert(n.id, spans);
                }
                handle_node_drag(n.id, n.pos, &result.response, cache, out);
            }
        });
}

/// Latch a drag anchor on the press frame, then convert each frame's
/// cumulative `drag_delta` into an absolute `MoveNode` target. The
/// anchor's `pos` stays at the press-frame position even as `n.pos`
/// updates over the gesture, so integration error doesn't accumulate.
fn handle_node_drag(
    node_id: scenarium::prelude::NodeId,
    pos: Vec2,
    response: &palantir::Response,
    cache: &mut FrameCache,
    out: &mut FrameResult,
) {
    if response.drag_started() {
        cache.drag_anchor = Some(DragAnchor { node_id, pos });
    }
    let Some(delta) = response.drag_delta() else {
        return;
    };
    let Some(anchor) = cache.drag_anchor else {
        return;
    };
    if anchor.node_id != node_id {
        return;
    }
    out.push(Intent::MoveNode {
        node_id,
        to: anchor.pos + delta,
    });
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
