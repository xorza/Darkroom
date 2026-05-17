use glam::Vec2;
use palantir::{
    Background, Color, Configure, LineCap, LineJoin, Panel, Scroll, Shape, Sizing, Ui, WidgetId,
};
use scenarium::prelude::NodeId;
use std::collections::HashMap;

use crate::frame_result::FrameResult;
use crate::gui::node_ui::{NodePortSpans, NodeUI};
use crate::scene::Scene;

const CONN_WIDTH: f32 = 2.0;
const CANVAS_BG: u32 = 0x1e1e1e;
const CONN_COLOR: u32 = 0x9ec1ff;

/// Interframe handles for every port that was recorded last pass. We
/// stash the `WidgetId`s (not the resolved rects) and resolve them
/// fresh via [`Ui::response_for`] each time we draw connections —
/// that way the rect we read reflects whichever pass last completed
/// `post_record` (Pass A's arrange when Pass B is running for a
/// drag-triggered relayout, etc.).
///
/// Flat layout: `widget_ids` pools all port `WidgetId`s in
/// node-then-input-then-output order; `nodes` maps each `NodeId` to
/// the pair of `PortSpan`s slicing into the pool.
#[derive(Default, Debug)]
pub struct PortCache {
    pub widget_ids: Vec<WidgetId>,
    pub nodes: HashMap<NodeId, NodePortSpans>,
}

impl PortCache {
    pub fn clear(&mut self) {
        self.widget_ids.clear();
        self.nodes.clear();
    }
}

/// Canvas-level UI scope: owns the port-widget-id cache and the
/// `NodeUI` that renders every graph node. `frame` draws connections
/// (resolving port rects through `Ui::response_for`), then delegates
/// node rendering to `NodeUI::draw_all` which refills the cache for
/// the next pass / frame.
#[derive(Default, Debug)]
pub struct GraphUI {
    pub ports: PortCache,
    pub node_ui: NodeUI,
}

impl GraphUI {
    /// Pre-record pass — see
    /// [`crate::gui::node_ui::NodeUI::prepass`].
    pub fn prepass(&mut self, ui: &Ui, out: &mut FrameResult) {
        self.node_ui.prepass(ui, out);
    }

    pub fn frame(&mut self, ui: &mut Ui, scene: &Scene, out: &mut FrameResult) {
        let Self { ports, node_ui } = self;
        // Outer Scroll provides pan / wheel-zoom / pinch — the inner
        // Canvas hosts absolutely-positioned nodes and the connection
        // beziers, sized large enough that all nodes fit inside its
        // bounds (Scroll's content-extent is the canvas's outer rect).
        Scroll::both()
            .id_salt("graph.scroll")
            .with_zoom()
            .hide_bars()
            .size((Sizing::FILL, Sizing::FILL))
            .background(Background {
                fill: Color::hex(CANVAS_BG).into(),
                ..Default::default()
            })
            .show(ui, |ui| {
                Panel::canvas()
                    .id_salt("graph.canvas")
                    .size((Sizing::Hug, Sizing::Hug))
                    .show(ui, |ui| {
                        draw_connections(ui, scene, ports);
                        ports.clear();
                        node_ui.draw_all(ui, scene, ports, out);
                    });
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
        let (Some(&src_wid), Some(&tgt_wid)) = (
            ports.widget_ids[src.outputs.range()].get(c.src_port),
            ports.widget_ids[tgt.inputs.range()].get(c.tgt_port),
        ) else {
            continue;
        };
        let (Some(p0), Some(p3)) = (port_center(ui, src_wid), port_center(ui, tgt_wid)) else {
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

fn port_center(ui: &Ui, wid: WidgetId) -> Option<Vec2> {
    ui.response_for(wid).rect.map(|r| r.center())
}
