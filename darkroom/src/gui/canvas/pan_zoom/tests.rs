use palantir::internals::UiHarness;
use scenarium::NodeId;

use super::*;
use crate::gui::scene::internals::{SceneFixture, scene_node_stub};

#[test]
fn node_bounds_uses_cached_sizes_and_falls_back_to_points() {
    // Regression for "Show all leaves nodes offscreen": node extents
    // must come from the cross-frame size cache, because a culled
    // (off-screen) node records no response the frame the button is
    // pressed. Three nodes:
    //   a: (0,0) 150×80      — on-screen, size cached
    //   b: (1000,500) 200×100 — culled, but its size is still cached
    //   c: (-50,300) never measured — contributes a point
    let (a, b, c) = (NodeId::unique(), NodeId::unique(), NodeId::unique());
    let mut arena = UiHarness::arena();
    let scene = SceneFixture::with_nodes([
        scene_node_stub(arena.ui(), a, Vec2::new(0.0, 0.0)),
        scene_node_stub(arena.ui(), b, Vec2::new(1000.0, 500.0)),
        scene_node_stub(arena.ui(), c, Vec2::new(-50.0, 300.0)),
    ]);
    let mut geometry = CanvasGeometry::default();
    geometry.seed_node_size(a, Size::new(150.0, 80.0));
    geometry.seed_node_size(b, Size::new(200.0, 100.0));

    // Union: min = c's x / a's y = (-50, 0); max = b's far corner
    // (1000+200, 500+100) = (1200, 600) → size (1250, 600). Without
    // the cache, b would count as a point and max.x would be 1000 —
    // its whole 200×100 body left outside the fit.
    let all = node_bounds(&geometry, scene.only_pane(), false).unwrap();
    assert_eq!(all.min, Vec2::new(-50.0, 0.0));
    assert_eq!(all.size, Size::new(1250.0, 600.0));

    // selected_only filters to exactly the selected node's rect.
    let scene = scene.with_selection([b]);
    let sel = node_bounds(&geometry, scene.only_pane(), true).unwrap();
    assert_eq!(sel.min, Vec2::new(1000.0, 500.0));
    assert_eq!(sel.size, Size::new(200.0, 100.0));

    // Empty scene → nothing to frame.
    let empty = SceneFixture::with_nodes([]);
    assert!(node_bounds(&geometry, empty.only_pane(), false).is_none());
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
    let (mut pan, mut zoom) = (Vec2::new(40.0, 20.0), 1.5);
    let pivot = Vec2::new(200.0, 150.0);
    let world_before = (pivot - pan) / zoom;
    zoom_about(
        &mut pan,
        &mut zoom,
        pivot,
        1.3,
        CANVAS_MIN_ZOOM,
        CANVAS_MAX_ZOOM,
    );
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
    zoom_about(
        &mut pan,
        &mut zoom,
        pivot,
        factor,
        CANVAS_MIN_ZOOM,
        CANVAS_MAX_ZOOM,
    );
    let world_after = (pivot - pan) / zoom;
    let drift = (world_after - world_before).length();
    assert!(drift < 1e-4, "drift {drift}");
    // Sanity: zoom did move in the expected direction (in).
    assert!(zoom > 0.8, "scroll up should grow zoom; got {zoom}");
}

#[test]
fn zoom_about_clamps_to_max() {
    // Trying to zoom past `CANVAS_MAX_ZOOM` saturates without
    // overshooting. Pivot invariance still holds at the clamped
    // value (effective factor = CANVAS_MAX_ZOOM / zoom_before, not the
    // requested factor).
    let (mut pan, mut zoom) = (Vec2::new(10.0, 10.0), CANVAS_MAX_ZOOM * 0.9);
    let pivot = Vec2::new(100.0, 100.0);
    zoom_about(
        &mut pan,
        &mut zoom,
        pivot,
        5.0,
        CANVAS_MIN_ZOOM,
        CANVAS_MAX_ZOOM,
    );
    assert!(
        (zoom - CANVAS_MAX_ZOOM).abs() < 1e-5,
        "expected saturation at CANVAS_MAX_ZOOM={CANVAS_MAX_ZOOM}, got {zoom}",
    );
}

#[test]
fn zoom_about_clamps_to_min() {
    let (mut pan, mut zoom) = (Vec2::new(10.0, 10.0), CANVAS_MIN_ZOOM * 1.1);
    let pivot = Vec2::new(100.0, 100.0);
    zoom_about(
        &mut pan,
        &mut zoom,
        pivot,
        0.01,
        CANVAS_MIN_ZOOM,
        CANVAS_MAX_ZOOM,
    );
    assert!(
        (zoom - CANVAS_MIN_ZOOM).abs() < 1e-5,
        "expected saturation at CANVAS_MIN_ZOOM={CANVAS_MIN_ZOOM}, got {zoom}",
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
    // A bbox far larger than any reachable zoom saturates at CANVAS_MIN_ZOOM
    // rather than the (smaller) exact fit; still centered.
    let bounds = Rect::new(0.0, 0.0, 100_000.0, 100_000.0);
    let viewport = Vec2::new(800.0, 600.0);
    let t = fit_target(bounds, viewport);
    assert!((t.zoom - CANVAS_MIN_ZOOM).abs() < 1e-6, "zoom {}", t.zoom);
    assert_centered(&t, bounds, viewport);
}

#[test]
fn zoom_about_ignores_non_positive_or_non_finite_factor() {
    // Defensive: pathological factors leave the viewport unchanged.
    let pan0 = Vec2::new(5.0, 7.0);
    let zoom0 = 1.25;
    for bad in [0.0_f32, -0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let (mut pan, mut zoom) = (pan0, zoom0);
        zoom_about(
            &mut pan,
            &mut zoom,
            Vec2::new(50.0, 50.0),
            bad,
            CANVAS_MIN_ZOOM,
            CANVAS_MAX_ZOOM,
        );
        assert_eq!(pan, pan0, "pan moved on bad factor {bad}");
        assert_eq!(zoom, zoom0, "zoom moved on bad factor {bad}");
    }
}

/// The pan gesture's three edges: an unlatched slot ignores everything, a
/// latched one measures from the latch rather than integrating, and a
/// `None` delta is the release that ends it.
#[test]
fn a_pan_drag_measures_from_its_latch_and_releases_once() {
    let mut anchor: GestureSlot<Vec2> = GestureSlot::default();

    // Before any latch, a delta drives nothing — `emit_pan_zoom` calls in
    // every frame, most of them with no gesture in flight.
    let mut unlatched = Vec2::ZERO;
    fold_pan_drag(&mut anchor, Some(Vec2::new(99.0, 99.0)), &mut unlatched);
    assert_eq!(unlatched, Vec2::ZERO, "an idle slot cannot pan");

    let start = Vec2::new(100.0, 40.0);
    anchor.latch(start);
    let mut pan = start;

    // Frame 1: start + delta.
    fold_pan_drag(&mut anchor, Some(Vec2::new(10.0, -5.0)), &mut pan);
    assert_eq!(pan, Vec2::new(110.0, 35.0), "start + delta");

    // Frame 2, larger travel: measured from the *latch*, so this is
    // start + the new total (130, 28), not frame 1's result + the new
    // delta (140, 23) that integrating would give.
    fold_pan_drag(&mut anchor, Some(Vec2::new(30.0, -12.0)), &mut pan);
    assert_eq!(pan, Vec2::new(130.0, 28.0), "start + total, not integrated");

    // Release, then a stray delta: the anchor is gone, so nothing moves.
    fold_pan_drag(&mut anchor, None, &mut pan);
    assert!(anchor.is_idle(), "a None delta ends the gesture");
    let mut after = pan;
    fold_pan_drag(&mut anchor, Some(Vec2::new(5.0, 5.0)), &mut after);
    assert_eq!(after, pan, "a released anchor drives nothing");
}
