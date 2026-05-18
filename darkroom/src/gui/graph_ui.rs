use glam::Vec2;
use palantir::{
    Background, Configure, LineCap, LineJoin, Panel, Sense, Shape, Sizing, TranslateScale, Ui,
    WidgetId,
};
use scenarium::prelude::NodeId;
use std::collections::HashMap;

use crate::frame_result::FrameResult;
use crate::gui::node_ui::{NodePortSpans, NodeUI};
use crate::scene::Scene;
use crate::theme::AppContext;

/// Bounds on the canvas zoom factor. Pinch deltas multiply in;
/// clamped each frame so pathological gestures can't drive it to 0
/// (which would make the inverse transform explode) or to a value so
/// large that the world coordinates underflow.
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 10.0;

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

/// Canvas-level UI scope: owns the port-widget-id cache, the
/// `NodeUI` that renders every graph node, and the manual pan/zoom
/// transform applied to the inner canvas. `frame` reads palantir's
/// pointer-event stream (drag on the outer canvas → pan, wheel/pinch
/// → zoom-about-cursor) and writes the result into [`Scene::pan`] /
/// [`Scene::zoom`], which then drive the inner canvas's
/// `TranslateScale`.
#[derive(Default, Debug)]
pub struct GraphUI {
    pub ports: PortCache,
    pub node_ui: NodeUI,
    /// `Scene::pan` snapshot captured at the frame the active pan-drag
    /// latched. While the drag is active, `scene.pan = anchor +
    /// drag_delta`. Lives on `GraphUI` because it's input bookkeeping
    /// (lifetime = one gesture), not viewport state.
    pan_anchor: Option<Vec2>,
}

impl GraphUI {
    /// Pre-record pass — see
    /// [`crate::gui::node_ui::NodeUI::prepass`].
    pub fn prepass(&mut self, ui: &Ui, ctx: &AppContext<'_>, scene: &Scene, out: &mut FrameResult) {
        self.node_ui.prepass(ui, ctx, scene, out);
    }

    pub fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &mut Scene,
        out: &mut FrameResult,
    ) {
        // Read outer's response before recording so this frame's
        // transform reflects the latest gesture — `response_for`
        // returns current-frame drag/scroll/pinch (palantir doc); no
        // 1-frame lag in the pan visual. Same pattern node drag uses
        // via `NodeUI::prepass`.
        self.apply_pan_zoom(ui, scene);

        let Self {
            ports,
            node_ui,
            pan_anchor: _,
        } = self;
        let pan_val = scene.pan;
        let zoom_val = scene.zoom;

        // Outer canvas: covers the whole pane, paints the canvas
        // background, owns the input routing for empty-canvas
        // gestures. Senses:
        // - `DRAG`: middle-button-style canvas pan (left-button drag).
        // - `SCROLL`: two-finger touchpad swipe / wheel = pan.
        // - `PINCH`: touchpad pinch = zoom-about-cursor.
        // Node panels (descendants of the *inner* canvas, which
        // carries the pan/zoom transform) hit-test first; only bare
        // canvas falls through to the outer's senses.
        Panel::canvas()
            .id(outer_canvas_widget_id())
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .background(Background {
                fill: ctx.theme.canvas_bg.into(),
                ..Default::default()
            })
            .show(ui, |ui| {
                Panel::canvas()
                    .id(inner_canvas_widget_id())
                    .size((Sizing::FILL, Sizing::FILL))
                    .transform(TranslateScale::new(pan_val, zoom_val))
                    .show(ui, |ui| {
                        // Inner canvas's pre-transform origin. Shapes
                        // and child node panels recorded inside this
                        // closure share the inner canvas's transform
                        // (palantir's `Panel::transform` applies to
                        // the body: child subtrees AND direct
                        // shapes), so port `layout_rect`s and bezier
                        // endpoints stay aligned at every zoom.
                        let canvas_origin = ui
                            .response_for(inner_canvas_widget_id())
                            .layout_rect
                            .map(|r| r.min)
                            .unwrap_or(Vec2::ZERO);
                        draw_connections(ui, ctx, scene, ports, canvas_origin);
                        ports.clear();
                        node_ui.draw_all(ui, ctx, scene, ports, out);
                    });
            });
    }

    /// Read the outer canvas's current-frame response and bake every
    /// pan/zoom gesture into `scene` so this frame's `TranslateScale`
    /// already reflects the gesture — no visible 1-frame lag. Mirrors
    /// `NodeUI::prepass`. Three independent sources:
    ///
    /// - **Drag** (`Sense::DRAG`): canvas-pan-on-empty. Anchor on
    ///   `drag_started`, then `pan = anchor + drag_delta` until release.
    /// - **Scroll** (`Sense::SCROLL`): two-finger touchpad swipe or
    ///   wheel. Palantir ingests the delta already-negated to "advance
    ///   offset forward" (input/mod.rs:167-178), so `pan -= delta`
    ///   matches Scroll's `transform.translation = -offset` convention.
    /// - **Pinch** (`Sense::PINCH`): zoom-about-cursor using the
    ///   `Response::pointer_local` pivot.
    fn apply_pan_zoom(&mut self, ui: &Ui, scene: &mut Scene) {
        let resp = ui.response_for(outer_canvas_widget_id());
        if resp.drag_started {
            self.pan_anchor = Some(scene.pan);
        }
        match (self.pan_anchor, resp.drag_delta) {
            (Some(anchor), Some(d)) => scene.pan = anchor + d,
            (Some(_), None) => self.pan_anchor = None,
            _ => {}
        }
        scene.pan -= resp.scroll_delta;
        if (resp.zoom_factor - 1.0).abs() > f32::EPSILON
            && let Some(pivot) = resp.pointer_local
        {
            zoom_about(scene, pivot, resp.zoom_factor);
        }
    }
}

/// Multiply `scene.zoom` by `factor` while holding the pre-transform
/// point under `pivot_local` fixed in the outer canvas. Standard
/// zoom-about-cursor algebra: world point under cursor =
/// (pivot - pan) / zoom; choose new pan so the same world point stays
/// under the same screen pixel after scaling.
fn zoom_about(scene: &mut Scene, pivot_local: Vec2, factor: f32) {
    if !factor.is_finite() || factor <= 0.0 {
        return;
    }
    let new_zoom = (scene.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    let effective = new_zoom / scene.zoom;
    scene.pan = pivot_local - (pivot_local - scene.pan) * effective;
    scene.zoom = new_zoom;
}

/// Stable id for the outer (pan-capture) canvas. `auto_stable` mixes
/// `file!()`/`line!()` so calls from different source lines stay
/// distinct; here we only need the id to survive between frames.
const fn outer_canvas_widget_id() -> WidgetId {
    WidgetId::auto_stable()
}

/// Stable id for the inner (transformed) canvas. Used both as the
/// widget seed and for resolving the canvas's pre-transform origin
/// in `port_center`.
const fn inner_canvas_widget_id() -> WidgetId {
    WidgetId::auto_stable()
}

fn draw_connections(
    ui: &mut Ui,
    ctx: &AppContext<'_>,
    scene: &Scene,
    ports: &PortCache,
    canvas_origin: Vec2,
) {
    let color = ctx.theme.connection;
    let width = ctx.theme.connection_width;
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
            width,
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
