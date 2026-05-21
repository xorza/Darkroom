use glam::Vec2;
use palantir::{
    Background, Configure, Panel, PointerButton, Rect, Sense, Sizing, TranslateScale, Ui, WidgetId,
};
use std::collections::HashMap;

use crate::app::AppContext;
use crate::gui::breaker::BreakerUI;
use crate::gui::connection_ui::ConnectionUI;
use crate::gui::new_node_ui::NewNodeUi;
use crate::gui::node_ui::{NodeUI, node_widget_id, port_circle_wid};
use crate::gui::{PortKind, PortRef};
use crate::intent::Intent;
use crate::scene::Scene;

/// Bounds on the canvas zoom factor. Pinch / scroll-zoom deltas
/// multiply in; clamped each frame so pathological gestures can't
/// drive it to 0 (which would make the inverse transform explode) or
/// to a value so large that the world coordinates underflow.
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 5.0;

/// Per-pixel base for converting wheel / touchpad scroll into a
/// multiplicative zoom factor. Tuned so a single classic wheel notch
/// (~16-20 logical px after palantir's line→pixel conversion) yields
/// roughly a 4-5% zoom step, while a fast touchpad swipe (~50-100 px
/// in one frame) stays a controlled ~13-22% step. Lower → slower
/// zoom, higher → snappier but jumps badly on touchpad.
const SCROLL_ZOOM_BASE: f32 = 1.0025;

/// Per-frame snapshot of the four `ResponseState` fields downstream
/// consumers actually read. Built once at the top of
/// [`GraphUI::frame`] by polling [`Ui::response_for`] on each port's
/// deterministic [`port_circle_wid`]. Sized to the four bytes-and-
/// bits we use (`layout_rect.center()`, `rect`, two edge bools)
/// instead of the full `ResponseState`.
///
/// Ports that haven't recorded yet (first frame after a node spawns)
/// have an entry with `layout_center` / `screen_rect` = `None`. The
/// edge bools default to `false` for them, so `drag_started` / `dragging`
/// queries are correct without a presence check.
#[derive(Default, Debug)]
pub struct PortFrame {
    map: HashMap<PortRef, PortInfo>,
}

#[derive(Clone, Copy, Debug, Default)]
struct PortInfo {
    /// Port-circle center in canvas-local (inner-canvas pre-transform)
    /// coords. Computed as `node.pos + port_offset_within_node` so a
    /// just-moved node's curves anchor on this frame's port positions
    /// instead of last frame's stale `response.layout_rect`. `None`
    /// when either the port or its parent node hasn't measured yet.
    layout_center: Option<Vec2>,
    /// Post-transform/clip screen rect for pointer hit-test (snap).
    /// Bypasses palantir's drag-capture hover suppression by reading
    /// geometry directly.
    screen_rect: Option<Rect>,
    /// `true` when the port should paint with its hover color. Filled
    /// from `response.hovered` in `rebuild`; an active connection
    /// drag's snap target gets it forced on via `set_hovered` after
    /// `ConnectionUI::apply` (palantir's drag-capture suppression
    /// otherwise hides the snap target from `response.hovered`).
    hovered: bool,
    /// One-frame edge: pointer-down → drag latched on this port this
    /// frame. Drives connection-drag start detection.
    drag_started: bool,
    /// Continuous: a drag is currently live on this port
    /// (`drag_delta` is `Some` OR `drag_started` fired this frame).
    /// Read on the start port to detect release.
    dragging: bool,
}

impl PortFrame {
    fn rebuild(&mut self, ui: &Ui, scene: &Scene) {
        self.map.clear();
        for n in &scene.nodes {
            // Port offsets within a node are stable; the node's
            // canvas-local position changes when the user drags. Take
            // `port_offset = port_rect.center - node_rect.min` from
            // last frame's layout (same frame for both, so any
            // ancestor-shared canvas-origin term cancels) and combine
            // with this frame's `n.pos` — curves anchor on the moved
            // node's *current* port positions, not last frame's.
            let node_min = ui
                .response_for(node_widget_id(n.id))
                .layout_rect
                .map(|r| r.min);
            let input_count = scene.ports(n.inputs).len();
            let output_count = scene.ports(n.outputs).len();
            for (kind, count) in [
                (PortKind::Input, input_count),
                (PortKind::Output, output_count),
            ] {
                for port_idx in 0..count {
                    let port = PortRef {
                        node_id: n.id,
                        kind,
                        port_idx,
                    };
                    let r = ui.response_for(port_circle_wid(port));
                    let layout_center = match (r.layout_rect, node_min) {
                        (Some(port_rect), Some(node_min)) => {
                            Some(n.pos + (port_rect.center() - node_min))
                        }
                        _ => None,
                    };
                    self.map.insert(
                        port,
                        PortInfo {
                            layout_center,
                            screen_rect: r.rect,
                            hovered: r.hovered,
                            drag_started: r.drag_started(),
                            dragging: r.drag_started() || r.drag_delta().is_some(),
                        },
                    );
                }
            }
        }
    }

    /// Canvas-local pre-transform port center. `None` when the port
    /// or its parent node hasn't been measured yet.
    pub(super) fn center_canvas_local(&self, p: PortRef) -> Option<Vec2> {
        self.map.get(&p)?.layout_center
    }

    /// `true` when `pointer` (screen coords) falls inside this port's
    /// post-transform/clip rect.
    pub(super) fn contains_pointer(&self, p: PortRef, pointer: Vec2) -> bool {
        self.map
            .get(&p)
            .and_then(|i| i.screen_rect)
            .is_some_and(|r| r.contains(pointer))
    }

    /// `true` on the one-frame edge of a drag-start on this port.
    pub(super) fn drag_started(&self, p: PortRef) -> bool {
        self.map.get(&p).is_some_and(|i| i.drag_started)
    }

    /// `true` while a drag started on this port is still live.
    pub(super) fn dragging(&self, p: PortRef) -> bool {
        self.map.get(&p).is_some_and(|i| i.dragging)
    }

    /// `true` when the port should paint with its hover color —
    /// `response.hovered` plus any forced-on override.
    pub(super) fn is_hovered(&self, p: PortRef) -> bool {
        self.map.get(&p).is_some_and(|i| i.hovered)
    }

    /// Force the hover flag on (idempotent). Called after
    /// `ConnectionUI::apply` for the active snap target so it lights
    /// up even though palantir's drag-capture suppression hides it
    /// from `response.hovered`.
    pub(super) fn set_hovered(&mut self, p: PortRef) {
        if let Some(info) = self.map.get_mut(&p) {
            info.hovered = true;
        }
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
    pub port_frame: PortFrame,
    pub node_ui: NodeUI,
    pub breaker_ui: BreakerUI,
    pub connection_ui: ConnectionUI,
    pub new_node_ui: NewNodeUi,
    /// `Scene::pan` snapshot captured at the frame the active pan-drag
    /// latched. While the drag is active, `scene.pan = anchor +
    /// drag_delta`. Lives on `GraphUI` because it's input bookkeeping
    /// (lifetime = one gesture), not viewport state.
    pan_anchor: Option<Vec2>,
}

impl GraphUI {
    /// Pre-record pass — see
    /// [`crate::gui::node_ui::NodeUI::prepass`].
    pub fn prepass(&mut self, ui: &mut Ui, scene: &Scene, out: &mut Vec<Intent>) {
        self.node_ui.prepass(ui, scene, out);
    }

    pub fn frame(
        &mut self,
        ui: &mut Ui,
        ctx: &AppContext<'_>,
        scene: &mut Scene,
        out: &mut Vec<Intent>,
    ) {
        // Read outer's response before recording so this frame's
        // transform reflects the latest gesture — `response_for`
        // returns current-frame drag/scroll/pinch (palantir doc); no
        // 1-frame lag in the pan visual. Same pattern node drag uses
        // via `NodeUI::prepass`.
        self.apply_pan_zoom(ui, scene);
        // Snapshot every port's response once for this frame — drag
        // detection, snap test, and bezier endpoints all read from the
        // same map instead of re-issuing `response_for` per use-site.
        self.port_frame.rebuild(ui, scene);
        self.breaker_ui.apply(ui, scene, out);
        self.connection_ui.apply(ui, scene, &self.port_frame, out);
        self.new_node_ui.apply(ui, ctx, scene, out);
        // Bake the snap target into `PortFrame.hovered` so node_ui's
        // port_row picks up the hover color via the same lookup it
        // uses for ordinary mouse-over. `response.hovered` is
        // suppressed on every widget except the drag-capture owner
        // while a drag is live, so without this override the
        // snapped-but-not-captured target stays at its idle color.
        if let Some(snap) = self.connection_ui.snap_port() {
            self.port_frame.set_hovered(snap);
        }

        let Self {
            port_frame,
            node_ui,
            breaker_ui,
            connection_ui,
            new_node_ui: _,
            pan_anchor: _,
        } = self;
        let pan_val = scene.pan;
        let zoom_val = scene.zoom;

        // Outer canvas: covers the whole pane, paints the canvas
        // background, owns the input routing for empty-canvas
        // gestures. Senses:
        // - `DRAG`: middle-button canvas pan (graph-editor
        //   convention; left-drag is reserved for rubber-band
        //   selection once that lands). Pulled via
        //   `Ui::drag_delta_by(.., PointerButton::Middle)`, since the
        //   left-only `ResponseState::drag_delta` doesn't carry middle.
        // - `SCROLL`: mouse wheel / touchpad swipe = zoom-about-cursor.
        // - `PINCH`: touchpad pinch = zoom-about-cursor.
        // Node panels (descendants of the *inner* canvas, which
        // carries the pan/zoom transform) hit-test first; only bare
        // canvas falls through to the outer's senses.
        //
        // `.clip_rect()` pins the inner-canvas subtree's `paint_rect`s
        // to the outer rect even when the inner transform zooms them
        // way past the viewport. Without it, at high zoom a single
        // off-screen node panel's screen rect can dwarf the surface,
        // damage threshold sees ratio ≫ 1 and trips `Damage::Full`
        // every pan/zoom tick.
        Panel::canvas()
            .id(outer_canvas_widget_id())
            .size((Sizing::FILL, Sizing::FILL))
            .sense(Sense::DRAG | Sense::SCROLL | Sense::PINCH)
            .clip_rect()
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
                        {
                            let mut probe = breaker_ui.probe(canvas_origin);
                            connection_ui.draw(ui, ctx, scene, port_frame, &mut probe);
                            node_ui.draw_all(ui, ctx, scene, port_frame, &mut probe);
                        }
                        breaker_ui.draw(ui, ctx);
                        connection_ui.draw_in_flight(ui, ctx, scene, port_frame, canvas_origin);
                    });
            });
    }

    /// Read the outer canvas's current-frame response and bake every
    /// pan/zoom gesture into `scene` so this frame's `TranslateScale`
    /// already reflects the gesture — no visible 1-frame lag. Mirrors
    /// `NodeUI::prepass`. Three independent sources:
    ///
    /// - **Middle-button drag** (`Sense::DRAG` +
    ///   `Ui::drag_delta_by`): canvas pan. Anchor on `drag_started_by`,
    ///   then `pan = anchor + delta` until release. Left-drag is
    ///   intentionally NOT routed to pan so it stays free for future
    ///   rubber-band selection.
    /// - **Scroll** (`Sense::SCROLL`): mouse wheel / touchpad swipe →
    ///   zoom-about-cursor (graph-editor convention: Figma / Blender
    ///   node editor / ComfyUI). Vertical delta only; horizontal is
    ///   ignored. Palantir ingests the scroll delta already-negated
    ///   so `+y` means "scroll content down" → zoom out, `-y` (wheel
    ///   up) → zoom in.
    /// - **Pinch** (`Sense::PINCH`): zoom-about-cursor using the
    ///   `Response::pointer_local` pivot.
    fn apply_pan_zoom(&mut self, ui: &Ui, scene: &mut Scene) {
        let resp = ui.response_for(outer_canvas_widget_id());
        if resp.drag_started_by(PointerButton::Middle) {
            self.pan_anchor = Some(scene.pan);
        }
        match (self.pan_anchor, resp.drag_delta_by(PointerButton::Middle)) {
            (Some(anchor), Some(d)) => scene.pan = anchor + d,
            (Some(_), None) => self.pan_anchor = None,
            _ => {}
        }
        if resp.scroll_pixels != Vec2::ZERO {
            scene.pan -= resp.scroll_pixels;
        }
        if resp.scroll_lines.y.abs() > f32::EPSILON
            && let Some(pivot) = resp.pointer_local
        {
            let line_px = ui.theme.text.line_height_for(ui.theme.text.font_size_px);
            zoom_about(
                scene,
                pivot,
                scroll_to_zoom_factor(resp.scroll_lines.y * line_px),
            );
        }
        if (resp.zoom_factor - 1.0).abs() > f32::EPSILON
            && let Some(pivot) = resp.pointer_local
        {
            zoom_about(scene, pivot, resp.zoom_factor);
        }
    }
}

/// Outer-canvas-local coords → inner-canvas pre-transform world
/// coords. Inner canvas applies `TranslateScale::new(pan, zoom)`,
/// so `outer = pan + zoom * world`.
pub(super) fn to_world(outer_local: Vec2, scene: &Scene) -> Vec2 {
    let zoom = if scene.zoom > 0.0 { scene.zoom } else { 1.0 };
    (outer_local - scene.pan) / zoom
}

/// Map a one-frame vertical scroll delta (in logical px, palantir's
/// "advance offset forward" sign convention — `+y` = scroll content
/// down) to a multiplicative zoom factor. Negative `delta_y` (wheel
/// up) zooms in (`factor > 1`); positive (wheel down) zooms out
/// (`factor < 1`). Pure function so it can be unit-tested without
/// spinning up a UI.
fn scroll_to_zoom_factor(delta_y: f32) -> f32 {
    SCROLL_ZOOM_BASE.powf(-delta_y)
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
pub(super) const fn outer_canvas_widget_id() -> WidgetId {
    WidgetId::auto_stable()
}

/// Stable id for the inner (transformed) canvas. Used as the widget
/// seed and for resolving the canvas's pre-transform origin in
/// connection draws.
const fn inner_canvas_widget_id() -> WidgetId {
    WidgetId::auto_stable()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Scene;

    fn scene_with(pan: Vec2, zoom: f32) -> Scene {
        Scene {
            pan,
            zoom,
            ..Scene::default()
        }
    }

    #[test]
    fn scroll_to_zoom_factor_zero_delta_is_identity() {
        // No scroll event → no zoom change. Bit-exact because
        // `1.0025_f32.powf(0.0)` returns exactly `1.0` by f32 spec.
        assert_eq!(scroll_to_zoom_factor(0.0), 1.0);
    }

    #[test]
    fn scroll_to_zoom_factor_wheel_up_zooms_in() {
        // One classic wheel notch up after palantir line→pixel
        // conversion lands around `-line_px` (theme default ≈ 18 px,
        // sign-flipped at ingest). Round-trip check: a typical wheel
        // notch produces a > 1.0 factor; magnitude is the documented
        // `SCROLL_ZOOM_BASE^|delta|`.
        let f = scroll_to_zoom_factor(-18.0);
        assert!(f > 1.0, "wheel up must zoom in, got factor {f}");
        // Hand-computed: 1.0025^18 ≈ 1.04604.
        let expected = SCROLL_ZOOM_BASE.powf(18.0);
        assert!(
            (f - expected).abs() < 1e-6,
            "factor {f} != expected {expected}",
        );
    }

    #[test]
    fn scroll_to_zoom_factor_wheel_down_zooms_out() {
        // Mirrored notch in the other direction: factor < 1, and
        // multiplied with the up-notch factor produces ~1.0 (the two
        // are reciprocals modulo float error).
        let f_down = scroll_to_zoom_factor(18.0);
        let f_up = scroll_to_zoom_factor(-18.0);
        assert!(f_down < 1.0, "wheel down must zoom out, got {f_down}");
        let product = f_down * f_up;
        assert!(
            (product - 1.0).abs() < 1e-6,
            "opposite-direction factors must reciprocate, got product {product}",
        );
    }

    #[test]
    fn scroll_to_zoom_factor_scales_monotonically_with_magnitude() {
        // 4 notches up zooms more aggressively than 1 notch up.
        let one = scroll_to_zoom_factor(-18.0);
        let four = scroll_to_zoom_factor(-72.0);
        assert!(
            four > one,
            "larger-magnitude up-scroll must produce larger factor; one={one}, four={four}",
        );
        // 4 × notch = (single notch factor) ^ 4 by exponent law.
        let expected_four = one.powi(4);
        assert!(
            (four - expected_four).abs() < 1e-5,
            "factor for 4 notches {four} != single^4 {expected_four}",
        );
    }

    #[test]
    fn zoom_about_holds_pivot_invariant() {
        // The point under the pivot in world space (i.e. in
        // pre-transform inner-canvas coords) must land on the same
        // local pivot after zooming. Algebra:
        //   world_before = (pivot - pan_before) / zoom_before
        //   world_after  = (pivot - pan_after)  / zoom_after
        //   require world_before == world_after.
        let mut scene = scene_with(Vec2::new(40.0, 20.0), 1.5);
        let pivot = Vec2::new(200.0, 150.0);
        let world_before = (pivot - scene.pan) / scene.zoom;
        zoom_about(&mut scene, pivot, 1.3);
        let world_after = (pivot - scene.pan) / scene.zoom;
        let drift = (world_after - world_before).length();
        assert!(
            drift < 1e-4,
            "world point under pivot drifted by {drift} (before={world_before}, after={world_after})",
        );
    }

    #[test]
    fn zoom_about_with_scroll_factor_preserves_pivot() {
        // End-to-end: a wheel scroll triggers `zoom_about` with the
        // `scroll_to_zoom_factor` output. The same pivot invariant
        // must hold regardless of which factor source the caller used.
        let mut scene = scene_with(Vec2::new(-15.0, 75.0), 0.8);
        let pivot = Vec2::new(300.0, 200.0);
        let world_before = (pivot - scene.pan) / scene.zoom;
        // 2 notches up.
        let factor = scroll_to_zoom_factor(-36.0);
        zoom_about(&mut scene, pivot, factor);
        let world_after = (pivot - scene.pan) / scene.zoom;
        let drift = (world_after - world_before).length();
        assert!(drift < 1e-4, "drift {drift}");
        // Sanity: zoom did move in the expected direction (in).
        assert!(
            scene.zoom > 0.8,
            "scroll up should grow zoom; got {}",
            scene.zoom
        );
    }

    #[test]
    fn zoom_about_clamps_to_max() {
        // Trying to zoom past `MAX_ZOOM` saturates without
        // overshooting. Pivot invariance still holds at the clamped
        // value (effective factor = MAX_ZOOM / zoom_before, not the
        // requested factor).
        let mut scene = scene_with(Vec2::new(10.0, 10.0), MAX_ZOOM * 0.9);
        let pivot = Vec2::new(100.0, 100.0);
        zoom_about(&mut scene, pivot, 5.0);
        assert!(
            (scene.zoom - MAX_ZOOM).abs() < 1e-5,
            "expected saturation at MAX_ZOOM={MAX_ZOOM}, got {}",
            scene.zoom,
        );
    }

    #[test]
    fn zoom_about_clamps_to_min() {
        let mut scene = scene_with(Vec2::new(10.0, 10.0), MIN_ZOOM * 1.1);
        let pivot = Vec2::new(100.0, 100.0);
        zoom_about(&mut scene, pivot, 0.01);
        assert!(
            (scene.zoom - MIN_ZOOM).abs() < 1e-5,
            "expected saturation at MIN_ZOOM={MIN_ZOOM}, got {}",
            scene.zoom,
        );
    }

    #[test]
    fn zoom_about_ignores_non_positive_or_non_finite_factor() {
        // Defensive: pathological factors leave scene unchanged.
        let pan = Vec2::new(5.0, 7.0);
        let zoom = 1.25;
        for bad in [0.0_f32, -0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut s = scene_with(pan, zoom);
            zoom_about(&mut s, Vec2::new(50.0, 50.0), bad);
            assert_eq!(s.pan, pan, "pan moved on bad factor {bad}");
            assert_eq!(s.zoom, zoom, "zoom moved on bad factor {bad}");
        }
    }
}
