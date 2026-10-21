use super::*;

/// The canvas's range — any range does, and this one has room on both sides
/// of 1:1.
const RANGE: ZoomRange = ZoomRange { min: 0.1, max: 5.0 };

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
    #[derive(Debug)]
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
            zoom: RANGE.max * 0.9,
            pivot: Vec2::new(100.0, 100.0),
            factor: 5.0,
            expected_zoom: RANGE.max,
        },
        Case {
            pan: Vec2::new(10.0, 10.0),
            zoom: RANGE.min * 1.1,
            pivot: Vec2::new(100.0, 100.0),
            factor: 0.01,
            expected_zoom: RANGE.min,
        },
    ];
    for Case {
        pan,
        zoom,
        pivot,
        factor,
        expected_zoom,
    } in cases
    {
        let world_before = (pivot - pan) / zoom;
        let mut v = Viewport { pan, zoom };
        RANGE.zoom_about(&mut v, pivot, factor);
        let Viewport { pan, zoom } = v;
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

#[test]
fn zoom_about_ignores_non_positive_or_non_finite_factor() {
    // Defensive: pathological factors leave the viewport unchanged.
    let pan0 = Vec2::new(5.0, 7.0);
    let zoom0 = 1.25;
    for bad in [0.0_f32, -0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut v = Viewport {
            pan: pan0,
            zoom: zoom0,
        };
        RANGE.zoom_about(&mut v, Vec2::new(50.0, 50.0), bad);
        let Viewport { pan, zoom } = v;
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
