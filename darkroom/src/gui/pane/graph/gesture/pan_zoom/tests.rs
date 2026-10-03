use super::*;
use crate::core::document::harness::DocFixture;
use crate::gui::graph_ctx::harness::GraphCtxFixture;

#[test]
fn node_bounds_uses_cached_sizes_and_falls_back_to_points() {
    // Regression for "Show all leaves nodes offscreen": node extents
    // must come from the cross-frame size cache, because a culled
    // (off-screen) node records no response the frame the button is
    // pressed. Three nodes:
    //   a: (0,0) 150×80      — on-screen, size cached
    //   b: (1000,500) 200×100 — culled, but its size is still cached
    //   c: (-50,300) never measured — contributes a point
    let mut fixture = DocFixture::default();
    let a = fixture.stub_at(Vec2::new(0.0, 0.0));
    let b = fixture.stub_at(Vec2::new(1000.0, 500.0));
    fixture.stub_at(Vec2::new(-50.0, 300.0));
    let mut scene = GraphCtxFixture::over(fixture);
    let mut geometry = CanvasGeometry::default();
    geometry.seed_node_size(a, Size::new(150.0, 80.0));
    geometry.seed_node_size(b, Size::new(200.0, 100.0));

    // Union: min = c's x / a's y = (-50, 0); max = b's far corner
    // (1000+200, 500+100) = (1200, 600) → size (1250, 600). Without
    // the cache, b would count as a point and max.x would be 1000 —
    // its whole 200×100 body left outside the fit.
    let all = node_bounds(&geometry, scene.graph_ctx(), false).unwrap();
    assert_eq!(all.min, Vec2::new(-50.0, 0.0));
    assert_eq!(all.size, Size::new(1250.0, 600.0));

    // selected_only filters to exactly the selected node's rect.
    let mut scene = scene.with_selection([b]);
    let sel = node_bounds(&geometry, scene.graph_ctx(), true).unwrap();
    assert_eq!(sel.min, Vec2::new(1000.0, 500.0));
    assert_eq!(sel.size, Size::new(200.0, 100.0));

    // Empty graph → nothing to frame.
    let mut empty = GraphCtxFixture::over(DocFixture::default());
    assert!(node_bounds(&geometry, empty.graph_ctx(), false).is_none());
}

/// Wheel up zooms in, wheel down zooms out, and each notch multiplies by the
/// same step. The expected values are `1.00250005722045898^n` in f64 — the
/// base is `1.0025` as f32 rounds it — so they test the formula, not repeat it.
#[test]
fn scroll_to_zoom_factor_is_one_step_per_pixel() {
    assert_eq!(scroll_to_zoom_factor(0.0), 1.0, "no scroll, no zoom");
    let cases: [(f32, f64); 4] = [
        (-18.0, 1.045_970_195_001_467_5),
        (18.0, 0.956_050_186_495_607_6),
        (-36.0, 1.094_053_648_831_408),
        (-72.0, 1.196_953_386_521_317_8),
    ];
    for (delta, expected) in cases {
        let got = f64::from(scroll_to_zoom_factor(delta));
        // `powf` is accurate to one ulp, not correctly rounded.
        assert!(
            (got - expected).abs() <= f64::from(f32::EPSILON) * expected,
            "scroll {delta}: factor {got}, expected {expected}",
        );
    }
}

/// The world point under the pivot stays under it, also when the zoom clamps
/// and the factor that applies is not the one asked for.
#[test]
fn zoom_about_holds_the_pivot_and_clamps() {
    struct Case {
        pan: Vec2,
        zoom: f32,
        pivot: Vec2,
        factor: f32,
        expected_zoom: f32,
    }
    let cases = [
        Case {
            pan: Vec2::new(40.0, 20.0),
            zoom: 1.5,
            pivot: Vec2::new(200.0, 150.0),
            factor: 1.3,
            expected_zoom: 1.5 * 1.3,
        },
        Case {
            pan: Vec2::new(-15.0, 75.0),
            zoom: 0.8,
            pivot: Vec2::new(300.0, 200.0),
            factor: scroll_to_zoom_factor(-36.0),
            expected_zoom: 0.8 * scroll_to_zoom_factor(-36.0),
        },
        Case {
            pan: Vec2::new(10.0, 10.0),
            zoom: CANVAS_MAX_ZOOM * 0.9,
            pivot: Vec2::new(100.0, 100.0),
            factor: 5.0,
            expected_zoom: CANVAS_MAX_ZOOM,
        },
        Case {
            pan: Vec2::new(10.0, 10.0),
            zoom: CANVAS_MIN_ZOOM * 1.1,
            pivot: Vec2::new(100.0, 100.0),
            factor: 0.01,
            expected_zoom: CANVAS_MIN_ZOOM,
        },
    ];
    for Case {
        mut pan,
        mut zoom,
        pivot,
        factor,
        expected_zoom,
    } in cases
    {
        let world_before = (pivot - pan) / zoom;
        zoom_about(
            &mut pan,
            &mut zoom,
            pivot,
            factor,
            CANVAS_MIN_ZOOM,
            CANVAS_MAX_ZOOM,
        );
        assert_eq!(zoom, expected_zoom, "factor {factor}");
        // Four f32 roundings lie between the two world points: the effective
        // factor, the scaled offset, and the two subtractions.
        let drift = ((pivot - pan) / zoom - world_before).abs().max_element();
        let tolerance = 4.0 * f32::EPSILON * world_before.abs().max_element();
        assert!(
            drift <= tolerance,
            "factor {factor}: the world point under the pivot drifted by {drift}",
        );
    }
}

/// Fitting puts the bounds' center on the pane's center, at the tighter
/// axis's scale, never past 1:1 and never under the minimum zoom. Every value
/// here is exact in f32, so the asserts are exact too.
#[test]
fn fit_target_centers_at_the_tighter_scale() {
    let pane = Vec2::new(800.0, 600.0);
    let cases = [
        // Margin 40 leaves 720×520: 720/1000 = 0.72 binds before 520/500.
        // pan = (400, 300) - (500, 250)·0.72 = (40, 120).
        (
            Rect::new(0.0, 0.0, 1000.0, 500.0),
            0.72,
            Vec2::new(40.0, 120.0),
        ),
        // It would fit at 5.2×, but stops at 1:1.
        // pan = (400, 300) - (50, 50) = (350, 250).
        (
            Rect::new(0.0, 0.0, 100.0, 100.0),
            1.0,
            Vec2::new(350.0, 250.0),
        ),
        // A zero-size box constrains no axis, so it is 1:1 and centred.
        // pan = (400, 300) - (200, 200) = (200, 100).
        (
            Rect::new(200.0, 200.0, 0.0, 0.0),
            1.0,
            Vec2::new(200.0, 100.0),
        ),
        // The exact fit is under the minimum zoom, so the minimum holds.
        // pan = (400, 300) - (50000, 50000)·0.1 = (-4600, -4700).
        (
            Rect::new(0.0, 0.0, 100_000.0, 100_000.0),
            CANVAS_MIN_ZOOM,
            Vec2::new(-4600.0, -4700.0),
        ),
    ];
    for (bounds, zoom, pan) in cases {
        let t = fit_target(bounds, pane);
        assert_eq!((t.zoom, t.pan), (zoom, pan), "{bounds:?}");
        assert_eq!(
            t.pan + bounds.center() * t.zoom,
            pane * 0.5,
            "{bounds:?} centred"
        );
    }
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

/// A frame scrolls when any of the three scroll channels carries input, and
/// a frame with none of them does not.
#[test]
fn a_frame_scrolls_on_any_scroll_channel() {
    let idle = ResponseState::default();
    assert!(!scrolled(&idle));

    let mut pixels = idle;
    pixels.scroll.pixels = Vec2::new(0.0, 3.0);
    let mut lines = idle;
    lines.scroll.lines.y = -1.0;
    let mut pinch = idle;
    pinch.scroll.zoom = ZoomFactor::new(1.1).unwrap();
    for resp in [pixels, lines, pinch] {
        assert!(scrolled(&resp), "{:?}", resp.scroll);
    }

    // A horizontal wheel delta drives nothing here, so it is no scroll.
    let mut sideways = idle;
    sideways.scroll.lines.x = 1.0;
    assert!(!scrolled(&sideways));
}
