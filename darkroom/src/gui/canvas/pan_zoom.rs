//! Canvas viewport gesture: middle-drag pan, wheel/pinch zoom-about-
//! cursor, and the zoom-factor math. Split out of `graph_ui` so the
//! orchestration there isn't tangled with the (independently testable)
//! viewport algebra. The gesture emits `Intent::SetViewport`, so pan/zoom
//! rides the same undo path as every other edit.

use common::FloatExt;
use glam::Vec2;
use palantir::{PointerButton, Rect, Size, Ui};

use crate::core::document::Viewport;
use crate::core::edit::intent::Intent;
use crate::gui::canvas::{CanvasGesture, outer_canvas_widget_id};
use crate::gui::node::node_widget_id;
use crate::gui::scene::Scene;

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

/// Read the outer canvas's current-frame response, compute the
/// target viewport, and emit an `Intent::SetViewport` when it
/// changed. The intent (not a direct write) is the only thing that
/// mutates the document's viewport — so pan/zoom rides the same
/// undo path as every other edit, and the undo stack coalesces a
/// continuous gesture into one entry via `GestureKey::Viewport`.
/// `pan_anchor` is the caller's drag-anchor slot (input bookkeeping,
/// one gesture's lifetime). Three independent sources:
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
pub(crate) fn emit_pan_zoom(
    pan_anchor: &mut Option<Vec2>,
    ui: &Ui,
    scene: &Scene,
    gesture: Option<CanvasGesture>,
    out: &mut Vec<Intent>,
) {
    let resp = ui.response_for(outer_canvas_widget_id());
    let mut pan = scene.viewport.pan;
    let mut zoom = scene.viewport.zoom;
    // Pan latch comes from the central classification; continuation and
    // wheel/pinch zoom below read the response directly (not arbitration).
    if gesture == Some(CanvasGesture::Pan) {
        *pan_anchor = Some(scene.viewport.pan);
    }
    match (*pan_anchor, resp.drag_delta_by(PointerButton::Middle)) {
        (Some(anchor), Some(d)) => pan = anchor + d,
        (Some(_), None) => *pan_anchor = None,
        _ => {}
    }
    if resp.scroll_pixels != Vec2::ZERO {
        pan -= resp.scroll_pixels;
    }
    if resp.scroll_lines.y.abs() > f32::EPSILON
        && let Some(pivot) = resp.pointer_local
    {
        let line_px = ui.theme.text.line_height_for(ui.theme.text.font_size_px);
        zoom_about(
            &mut pan,
            &mut zoom,
            pivot,
            scroll_to_zoom_factor(resp.scroll_lines.y * line_px),
        );
    }
    if (resp.zoom_factor - 1.0).abs() > f32::EPSILON
        && let Some(pivot) = resp.pointer_local
    {
        zoom_about(&mut pan, &mut zoom, pivot, resp.zoom_factor);
    }
    // Only emit when the gesture actually moved the viewport
    // (approx compare — exact float `!=` would emit on sub-epsilon
    // jitter). The `SetViewport` undo step is also `is_noop`-
    // filtered in `drain_intents`; this just skips the build on
    // idle frames.
    let unchanged =
        pan.approximately_eq(scene.viewport.pan) && zoom.approximately_eq(scene.viewport.zoom);
    if !unchanged {
        out.push(Intent::SetViewport {
            to: Viewport { pan, zoom },
        });
    }
}

/// Multiply `zoom` by `factor` while holding the pre-transform point
/// under `pivot_local` fixed in the outer canvas. Operates on the
/// caller's local `(pan, zoom)` so the gesture can fold several inputs
/// before emitting one `Intent::SetViewport`. Standard zoom-about-
/// cursor algebra: world point under cursor = `(pivot - pan) / zoom`;
/// choose new pan so that same world point stays under the same screen
/// pixel after scaling. Clamps to `[MIN_ZOOM, MAX_ZOOM]`; ignores
/// non-finite / non-positive factors.
fn zoom_about(pan: &mut Vec2, zoom: &mut f32, pivot_local: Vec2, factor: f32) {
    if !factor.is_finite() || factor <= 0.0 {
        return;
    }
    let new_zoom = (*zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    let effective = new_zoom / *zoom;
    *pan = pivot_local - (pivot_local - *pan) * effective;
    *zoom = new_zoom;
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

/// Breathing room (logical px) left on every side when fitting content
/// to the viewport, so framed nodes don't butt against the pane edge.
const FIT_MARGIN: f32 = 40.0;

/// A one-shot viewport-framing request from the graph toolbar. Each
/// resolves to an `Intent::SetViewport`, so a reframe rides the same
/// undo path as a manual pan/zoom (and coalesces with it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ViewAction {
    /// Reset to 1:1 zoom, centered on all nodes (world origin when empty).
    Reset,
    /// Fit every node in the view.
    ShowAll,
    /// Fit the selected nodes; a no-op when nothing is selected.
    ShowSelected,
}

/// Compute the `SetViewport` intent a [`ViewAction`] implies, or `None`
/// when there's nothing to frame — `ShowSelected` with an empty
/// selection, no nodes to fit, or the viewport not yet measured. Reads
/// last frame's node/canvas geometry via `Ui::response_for`; node world
/// size comes from the body's pre-transform `layout_rect`, position from
/// `SceneNode::pos`.
pub(crate) fn view_action_intent(ui: &Ui, scene: &Scene, action: ViewAction) -> Option<Intent> {
    let vp = ui.response_for(outer_canvas_widget_id()).layout_rect?.size;
    let pane = Vec2::new(vp.w, vp.h);
    let to = match action {
        ViewAction::Reset => reset_target(ui, scene, pane),
        ViewAction::ShowAll => fit_target(node_bounds(ui, scene, false)?, pane),
        ViewAction::ShowSelected => {
            if scene.selected_nodes.is_empty() {
                return None;
            }
            fit_target(node_bounds(ui, scene, true)?, pane)
        }
    };
    Some(Intent::SetViewport { to })
}

/// 1:1 zoom, centered on all content (world origin when the graph is empty).
fn reset_target(ui: &Ui, scene: &Scene, pane: Vec2) -> Viewport {
    let pan = match node_bounds(ui, scene, false) {
        Some(b) => pane * 0.5 - b.center(),
        None => Vec2::ZERO,
    };
    Viewport { pan, zoom: 1.0 }
}

/// World-space (inner-canvas pre-transform) bounding box of the framed
/// nodes — every node, or only the selected ones. Position is
/// `SceneNode::pos` (document world coords); extent is the node body's
/// last-arranged `layout_rect` size (pre-transform, so unscaled), or a
/// point when it hasn't measured yet. `None` when no node qualifies.
fn node_bounds(ui: &Ui, scene: &Scene, selected_only: bool) -> Option<Rect> {
    let mut acc: Option<Rect> = None;
    for n in &scene.nodes {
        if selected_only && !scene.selected_nodes.contains(&n.id) {
            continue;
        }
        let size = ui
            .response_for(node_widget_id(n.id))
            .layout_rect
            .map(|r| r.size)
            .unwrap_or(Size::ZERO);
        let rect = Rect { min: n.pos, size };
        acc = Some(acc.map_or(rect, |a| a.union(rect)));
    }
    acc
}

/// Fit `bounds` (world coords) centered in a `viewport`-sized pane,
/// leaving [`FIT_MARGIN`] on every side. The scale is the tighter of the
/// two per-axis fits, never magnified past 1:1 (a lone small node
/// shouldn't balloon), and clamped to `[MIN_ZOOM, MAX_ZOOM]`. Placing the
/// bbox center at the viewport center uses the same `outer_local = pan +
/// scale * world` mapping the inner-canvas transform applies.
fn fit_target(bounds: Rect, pane: Vec2) -> Viewport {
    let avail_x = (pane.x - 2.0 * FIT_MARGIN).max(1.0);
    let avail_y = (pane.y - 2.0 * FIT_MARGIN).max(1.0);
    // A sub-pixel extent (single node, or a flat row/column) doesn't
    // constrain its axis — treat it as unbounded so the other axis wins.
    let sx = if bounds.size.w > 1.0 {
        avail_x / bounds.size.w
    } else {
        f32::INFINITY
    };
    let sy = if bounds.size.h > 1.0 {
        avail_y / bounds.size.h
    } else {
        f32::INFINITY
    };
    let zoom = sx.min(sy).min(1.0).clamp(MIN_ZOOM, MAX_ZOOM);
    let pan = pane * 0.5 - bounds.center() * zoom;
    Viewport { pan, zoom }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (mut pan, mut zoom) = (Vec2::new(40.0, 20.0), 1.5);
        let pivot = Vec2::new(200.0, 150.0);
        let world_before = (pivot - pan) / zoom;
        zoom_about(&mut pan, &mut zoom, pivot, 1.3);
        let world_after = (pivot - pan) / zoom;
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
        let (mut pan, mut zoom) = (Vec2::new(-15.0, 75.0), 0.8);
        let pivot = Vec2::new(300.0, 200.0);
        let world_before = (pivot - pan) / zoom;
        // 2 notches up.
        let factor = scroll_to_zoom_factor(-36.0);
        zoom_about(&mut pan, &mut zoom, pivot, factor);
        let world_after = (pivot - pan) / zoom;
        let drift = (world_after - world_before).length();
        assert!(drift < 1e-4, "drift {drift}");
        // Sanity: zoom did move in the expected direction (in).
        assert!(zoom > 0.8, "scroll up should grow zoom; got {zoom}");
    }

    #[test]
    fn zoom_about_clamps_to_max() {
        // Trying to zoom past `MAX_ZOOM` saturates without
        // overshooting. Pivot invariance still holds at the clamped
        // value (effective factor = MAX_ZOOM / zoom_before, not the
        // requested factor).
        let (mut pan, mut zoom) = (Vec2::new(10.0, 10.0), MAX_ZOOM * 0.9);
        let pivot = Vec2::new(100.0, 100.0);
        zoom_about(&mut pan, &mut zoom, pivot, 5.0);
        assert!(
            (zoom - MAX_ZOOM).abs() < 1e-5,
            "expected saturation at MAX_ZOOM={MAX_ZOOM}, got {zoom}",
        );
    }

    #[test]
    fn zoom_about_clamps_to_min() {
        let (mut pan, mut zoom) = (Vec2::new(10.0, 10.0), MIN_ZOOM * 1.1);
        let pivot = Vec2::new(100.0, 100.0);
        zoom_about(&mut pan, &mut zoom, pivot, 0.01);
        assert!(
            (zoom - MIN_ZOOM).abs() < 1e-5,
            "expected saturation at MIN_ZOOM={MIN_ZOOM}, got {zoom}",
        );
    }

    /// The world point at the bbox center must land on the viewport
    /// center after applying the fitted `pan`/`scale`
    /// (`outer_local = pan + scale * world`).
    fn assert_centered(t: &Viewport, bounds: Rect, pane: Vec2) {
        let mapped = t.pan + bounds.center() * t.zoom;
        let drift = (mapped - pane * 0.5).length();
        assert!(drift < 1e-3, "bbox center off viewport center by {drift}");
    }

    #[test]
    fn fit_target_shrinks_oversized_bounds() {
        // 1000×500 world bbox at origin into an 800×600 pane. Margin 40 →
        // avail 720×520. sx = 720/1000 = 0.72, sy = 520/500 = 1.04; the
        // width binds, so scale = 0.72.
        let bounds = Rect::new(0.0, 0.0, 1000.0, 500.0);
        let viewport = Vec2::new(800.0, 600.0);
        let t = fit_target(bounds, viewport);
        assert!((t.zoom - 0.72).abs() < 1e-4, "zoom {}", t.zoom);
        // pan = (400,300) - (500,250)*0.72 = (40, 120).
        assert!(
            (t.pan - Vec2::new(40.0, 120.0)).length() < 1e-3,
            "pan {}",
            t.pan
        );
        assert_centered(&t, bounds, viewport);
    }

    #[test]
    fn fit_target_never_magnifies_past_one_to_one() {
        // A small bbox would fit at 5.2×, but fitting must not zoom in
        // past 1:1 — scale caps at 1.0, still centered.
        let bounds = Rect::new(0.0, 0.0, 100.0, 100.0);
        let viewport = Vec2::new(800.0, 600.0);
        let t = fit_target(bounds, viewport);
        assert_eq!(t.zoom, 1.0);
        // pan = (400,300) - (50,50)*1.0 = (350, 250).
        assert!(
            (t.pan - Vec2::new(350.0, 250.0)).length() < 1e-3,
            "pan {}",
            t.pan
        );
        assert_centered(&t, bounds, viewport);
    }

    #[test]
    fn fit_target_degenerate_point_holds_scale_and_centers() {
        // A zero-size bbox (single unmeasured node) can't fit-scale — both
        // axes are unbounded → scale falls back to 1.0, point recentred.
        let bounds = Rect::new(200.0, 200.0, 0.0, 0.0);
        let viewport = Vec2::new(800.0, 600.0);
        let t = fit_target(bounds, viewport);
        assert_eq!(t.zoom, 1.0);
        assert!(
            (t.pan - Vec2::new(200.0, 100.0)).length() < 1e-3,
            "pan {}",
            t.pan
        );
        assert_centered(&t, bounds, viewport);
    }

    #[test]
    fn fit_target_clamps_to_min_zoom() {
        // A bbox far larger than any reachable zoom saturates at MIN_ZOOM
        // rather than the (smaller) exact fit; still centered.
        let bounds = Rect::new(0.0, 0.0, 100_000.0, 100_000.0);
        let viewport = Vec2::new(800.0, 600.0);
        let t = fit_target(bounds, viewport);
        assert!((t.zoom - MIN_ZOOM).abs() < 1e-6, "zoom {}", t.zoom);
        assert_centered(&t, bounds, viewport);
    }

    #[test]
    fn zoom_about_ignores_non_positive_or_non_finite_factor() {
        // Defensive: pathological factors leave the viewport unchanged.
        let pan0 = Vec2::new(5.0, 7.0);
        let zoom0 = 1.25;
        for bad in [0.0_f32, -0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let (mut pan, mut zoom) = (pan0, zoom0);
            zoom_about(&mut pan, &mut zoom, Vec2::new(50.0, 50.0), bad);
            assert_eq!(pan, pan0, "pan moved on bad factor {bad}");
            assert_eq!(zoom, zoom0, "zoom moved on bad factor {bad}");
        }
    }
}
