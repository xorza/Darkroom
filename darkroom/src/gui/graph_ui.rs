use glam::Vec2;
use palantir::{
    Background, Color, Configure, LineCap, LineJoin, Panel, Scroll, Shape, Sizing, Spacing, Ui,
    WidgetId,
};
use scenarium::prelude::NodeId;
use std::collections::HashMap;

use crate::frame_result::FrameResult;
use crate::gui::node_ui::{NodePortSpans, NodeUI};
use crate::scene::Scene;

const CONN_WIDTH: f32 = 2.0;
const CANVAS_BG: u32 = 0x1e1e1e;
const CONN_COLOR: u32 = 0x9ec1ff;
/// Extra pan slack on every side of the node bbox, so the user always
/// has room to drag a node past the current leading/trailing edge
/// without immediately hitting the offset clamp. Reapplied every
/// frame on top of the bbox-driven margin so the slack stays
/// consistent as nodes move.
const CANVAS_SLACK: f32 = 400.0;

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
        // beziers. Canvas hugs the bounding box of its **positive**
        // children only (palantir's `Sizing::Hug` rule); negatively-
        // placed nodes render outside the hugged rect and are
        // reachable through the dynamic `content_margin` below.
        //
        // The margin tracks the live scene bbox each frame: the
        // leading side grows by `|min(0, min node.pos)|` so dragging
        // a node past origin always leaves enough pan slack to scroll
        // back to it. Per-frame recompute is fine — there are at most
        // a few dozen nodes and `Spacing` is 8 bytes. No palantir
        // internals are involved; this is plain user-side wiring on
        // top of `Scroll::content_margin`.
        let mut bb_min = Vec2::ZERO;
        for n in &scene.nodes {
            bb_min = bb_min.min(n.pos);
        }
        let margin = Spacing::new(
            -bb_min.x.min(0.0) + CANVAS_SLACK,
            -bb_min.y.min(0.0) + CANVAS_SLACK,
            CANVAS_SLACK,
            CANVAS_SLACK,
        );
        Scroll::both()
            .id_salt("graph.scroll")
            .with_zoom()
            .hide_bars()
            .content_margin(margin)
            .size((Sizing::FILL, Sizing::FILL))
            .background(Background {
                fill: Color::hex(CANVAS_BG).into(),
                ..Default::default()
            })
            .show(ui, |ui| {
                // Canvas top-left in *pre-transform* world coords.
                // `response_for(...).rect` is the post-cascade visible
                // rect (clipped by the Scroll viewport), so its `min`
                // jumps when the canvas pans partly off-screen. The
                // pre-transform layout rect is unclipped and not
                // affected by Scroll's transform — exactly what shape
                // owner-local geometry expects.
                let canvas_origin = ui
                    .response_for(canvas_widget_id())
                    .layout_rect
                    .map(|r| r.min)
                    .unwrap_or(Vec2::ZERO);
                Panel::canvas()
                    .id(canvas_widget_id())
                    .size((Sizing::Hug, Sizing::Hug))
                    .show(ui, |ui| {
                        draw_connections(ui, scene, ports, canvas_origin);
                        ports.clear();
                        node_ui.draw_all(ui, scene, ports, out);
                    });
            });
    }
}

/// Verbatim `WidgetId` for the graph canvas. Hashed once from a
/// static string so the id is stable across frames and reachable
/// from `Ui::response_for` without round-tripping through palantir's
/// parent-mixed `make_persistent_id` chain.
const fn canvas_widget_id() -> WidgetId {
    WidgetId::auto_stable()
}

fn draw_connections(ui: &mut Ui, scene: &Scene, ports: &PortCache, canvas_origin: Vec2) {
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
        let (Some(p0), Some(p3)) = (
            port_center(ui, src_wid, canvas_origin),
            port_center(ui, tgt_wid, canvas_origin),
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

fn port_center(ui: &Ui, wid: WidgetId, canvas_origin: Vec2) -> Option<Vec2> {
    // Same as `canvas_origin`: use the unclipped, pre-transform layout
    // rect so the result is owner-local in the canvas's pre-transform
    // frame, which is what `Shape::CubicBezier` polyline coords expect.
    ui.response_for(wid)
        .layout_rect
        .map(|r| r.center() - canvas_origin)
}
