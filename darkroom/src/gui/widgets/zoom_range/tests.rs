use glam::UVec2;
use palantir::{Harness, Rect};

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
/// and the factor that applies is not the one asked for. A zoom outside the
/// range steps back toward it and never away.
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
        // Above the range, as a fit to a tiny image is: a step in holds, a step out moves.
        Case {
            pan: Vec2::new(10.0, 10.0),
            zoom: 8.0,
            pivot: Vec2::new(100.0, 100.0),
            factor: 1.25,
            expected_zoom: 8.0,
        },
        Case {
            pan: Vec2::new(10.0, 10.0),
            zoom: 8.0,
            pivot: Vec2::new(100.0, 100.0),
            factor: 0.5,
            expected_zoom: 4.0,
        },
        // Below it, as a fit to a huge image is: a step out holds, a step in moves.
        Case {
            pan: Vec2::new(10.0, 10.0),
            zoom: 0.05,
            pivot: Vec2::new(100.0, 100.0),
            factor: 0.5,
            expected_zoom: 0.05,
        },
        Case {
            pan: Vec2::new(10.0, 10.0),
            zoom: 0.05,
            pivot: Vec2::new(100.0, 100.0),
            factor: 1.5,
            expected_zoom: 0.075,
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
    let mut notches = idle;
    notches.scroll.notches.y = -1.0;
    let mut sideways = idle;
    sideways.scroll.notches.x = 1.0;
    let mut pinch = idle;
    pinch.scroll.zoom = ZoomFactor::new(1.1).unwrap();
    for resp in [pixels, notches, sideways, pinch] {
        assert!(scrolled(&resp), "{:?}", resp.scroll);
    }
}

/// A frame reads the response's delta, pointer and rect and the theme's line height, and the
/// command key alone selects the zoom: Shift, which turns a wheel sideways, does not.
#[test]
fn a_scroll_frame_reads_its_surface_and_the_command_key() {
    let delta = ScrollDelta {
        pixels: Vec2::new(1.0, 2.0),
        ..ScrollDelta::default()
    };
    let mut resp = ResponseState {
        scroll: delta,
        pointer_local: Some(Vec2::new(3.0, 4.0)),
        layout_rect: Some(Rect::new(5.0, 6.0, 70.0, 80.0)),
        ..ResponseState::default()
    };

    let mut h = Harness::new(UVec2::new(100, 100));
    let line_px = h.ui().theme().text.line_height();
    for (mods, action) in [
        (Modifiers::NONE, ScrollAction::Pan),
        (Modifiers::SHIFT, ScrollAction::Pan),
        (Modifiers::CTRL, ScrollAction::Zoom),
        (Modifiers::CTRL | Modifiers::SHIFT, ScrollAction::Zoom),
    ] {
        h.hold_modifiers(mods);
        let frame = ScrollFrame::of(h.ui(), &resp);
        assert_eq!(frame.action, action, "{mods:?}");
        assert_eq!(frame.delta, delta);
        assert_eq!(frame.pivot, Some(Vec2::new(3.0, 4.0)));
        assert_eq!(frame.pane, Size::new(70.0, 80.0));
        assert_eq!(frame.line_px, line_px);
    }

    resp.layout_rect = None;
    assert_eq!(ScrollFrame::of(h.ui(), &resp).pane, Size::ZERO);
}

/// A frame at the viewport's start, 20 px a line, in an 800 × 600 pane.
fn frame(action: ScrollAction, delta: ScrollDelta) -> ScrollFrame {
    ScrollFrame {
        delta,
        pivot: Some(Vec2::new(100.0, 50.0)),
        pane: Size::new(800.0, 600.0),
        line_px: 20.0,
        action,
    }
}

const START: Viewport = Viewport {
    pan: Vec2::new(10.0, 20.0),
    zoom: 2.0,
};

fn folded(frame: ScrollFrame) -> Viewport {
    let mut v = START;
    RANGE.fold_scroll(&mut v, frame);
    v
}

fn zoomed(factor: f32) -> Viewport {
    let mut v = START;
    RANGE.zoom_about(&mut v, Vec2::new(100.0, 50.0), factor);
    v
}

/// With no command key every unit pans, on both axes, whatever device sent it: pixels as they
/// are, lines at 20 px, a page at max(0.875 × 600, 600 − 40) = 560 px. The zoom does not move.
#[test]
fn a_scroll_pans_in_every_unit() {
    let rows = [
        (
            ScrollDelta {
                pixels: Vec2::new(3.0, 4.0),
                ..ScrollDelta::default()
            },
            Vec2::new(7.0, 16.0),
        ),
        (
            ScrollDelta {
                lines: Vec2::new(1.0, -2.0),
                notches: Vec2::new(1.0, -2.0),
                ..ScrollDelta::default()
            },
            Vec2::new(-10.0, 60.0),
        ),
        (
            ScrollDelta {
                notches: Vec2::new(0.0, 1.0),
                pages: Vec2::new(0.0, 1.0),
                ..ScrollDelta::default()
            },
            Vec2::new(10.0, -540.0),
        ),
        (
            ScrollDelta {
                notches: Vec2::new(0.0, 1.0),
                ..ScrollDelta::default()
            },
            START.pan,
        ),
    ];
    for (delta, pan) in rows {
        assert_eq!(
            folded(frame(ScrollAction::Pan, delta)),
            Viewport { pan, zoom: 2.0 },
            "{delta:?}",
        );
    }
}

/// Under the command key a scroll zooms about the pointer and pans nothing else: a notch is 20 px
/// of zoom, and a setting of three lines per notch leaves that step alone.
#[test]
fn a_scroll_under_the_command_key_zooms_by_notches() {
    let notch = ScrollDelta {
        notches: Vec2::new(0.0, -1.0),
        lines: Vec2::new(0.0, -1.0),
        ..ScrollDelta::default()
    };
    let three_lines = ScrollDelta {
        lines: Vec2::new(0.0, -3.0),
        ..notch
    };
    let one_step = zoomed(scroll_to_zoom_factor(-20.0));
    assert_ne!(one_step, START);
    assert_eq!(folded(frame(ScrollAction::Zoom, notch)), one_step);
    assert_eq!(folded(frame(ScrollAction::Zoom, three_lines)), one_step);

    let pixels = ScrollDelta {
        pixels: Vec2::new(0.0, 8.0),
        ..ScrollDelta::default()
    };
    assert_eq!(
        folded(frame(ScrollAction::Zoom, pixels)),
        zoomed(scroll_to_zoom_factor(8.0)),
    );

    // A sideways scroll has no step to zoom by.
    let sideways = ScrollDelta {
        notches: Vec2::new(1.0, 0.0),
        lines: Vec2::new(1.0, 0.0),
        ..ScrollDelta::default()
    };
    assert_eq!(folded(frame(ScrollAction::Zoom, sideways)), START);

    // With the pointer off the surface there is no pivot to zoom about.
    let off = ScrollFrame {
        pivot: None,
        ..frame(ScrollAction::Zoom, notch)
    };
    assert_eq!(folded(off), START);
}

/// A pinch zooms about the pointer with or without the command key.
#[test]
fn a_pinch_zooms_under_either_action() {
    let pinch = ScrollDelta {
        zoom: ZoomFactor::new(1.5).unwrap(),
        ..ScrollDelta::default()
    };
    for action in [ScrollAction::Pan, ScrollAction::Zoom] {
        assert_eq!(folded(frame(action, pinch)), zoomed(1.5), "{action:?}");
    }
}
