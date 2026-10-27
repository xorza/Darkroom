use palantir::Size;

use super::*;

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "expected ~{expected}, got {actual}"
    );
}

/// A wire's chord is the straight distance between its ends, whatever its handles.
#[test]
fn a_chord_runs_end_to_end() {
    let wire = Wire::data(Vec2::new(0.0, 0.0), Vec2::new(30.0, 40.0));
    assert_eq!(wire.chord(), 50.0);
}

#[test]
fn data_wire_forward_span_uses_half_vertical_gap_clamped() {
    // Level ports (no vertical gap): clamps up to MIN_HANDLE.
    let w = Wire::data(Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0));
    assert_eq!(w.p1, Vec2::new(MIN_HANDLE, 0.0));
    assert_eq!(w.p2, Vec2::new(200.0 - MIN_HANDLE, 0.0));

    // A tall span's half-gap (300) exceeds MAX_HANDLE, so it clamps down.
    // Only the x component moves — both handles stay level with their
    // own endpoint's y.
    let w = Wire::data(Vec2::new(0.0, 0.0), Vec2::new(200.0, 600.0));
    assert_eq!(w.p1, Vec2::new(MAX_HANDLE, 0.0));
    assert_eq!(w.p2, Vec2::new(200.0 - MAX_HANDLE, 600.0));
}

#[test]
fn data_wire_backward_span_loops_via_sqrt_reach() {
    // p3 sits left of p0 by 400 — forward term is 0 (level), backward
    // term is 10 * sqrt(400) = 200, which wins.
    let w = Wire::data(Vec2::new(400.0, 0.0), Vec2::new(0.0, 0.0));
    assert_close(w.p1.x, 400.0 + 200.0);
    assert_close(w.p2.x, 0.0 - 200.0);
}

#[test]
fn event_wire_arrives_up_left_at_the_subscriber_pin() {
    // Level span of 100: d = max(100 * 0.4, MIN_HANDLE) = 40. The
    // emitter handle leaves straight right; the subscriber handle backs
    // off along the normalized (-1, -1) diagonal, i.e. 40/√2 ≈ 28.284
    // in each axis, so the curve meets the outward-pointing pin
    // head-on rather than flat.
    let w = Wire::event(Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0));
    assert_eq!(w.p1, Vec2::new(40.0, 0.0));
    let diag = 40.0 / 2.0_f32.sqrt();
    assert_close(w.p2.x, 100.0 - diag);
    assert_close(w.p2.y, -diag);

    // A short span floors at MIN_HANDLE (30) instead of collapsing:
    // 10 * 0.4 = 4 < 30.
    let short = Wire::event(Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0));
    assert_eq!(short.p1, Vec2::new(MIN_HANDLE, 0.0));

    // The two families place different handles for the same endpoints —
    // a data wire arrives level from the left, an event wire diagonally.
    assert_ne!(Wire::data(Vec2::ZERO, Vec2::new(100.0, 0.0)).p2, w.p2);
}

/// The curve runs from `p0` at `t = 0` to `p3` at `t = 1`, and at `t = ½`
/// weighs the four points `⅛, ⅜, ⅜, ⅛`: over `x = 0, 30, 60, 90` and
/// `y = 0, 40, 40, 0` that is `(0 + 11.25 + 22.5 + 11.25, 0 + 15 + 15 + 0)` =
/// `(45, 30)`, every term exact in f32.
#[test]
fn at_runs_the_curve_from_p0_to_p3() {
    let w = Wire {
        p0: Vec2::new(0.0, 0.0),
        p1: Vec2::new(30.0, 40.0),
        p2: Vec2::new(60.0, 40.0),
        p3: Vec2::new(90.0, 0.0),
    };
    assert_eq!(w.at(0.0), w.p0);
    assert_eq!(w.at(1.0), w.p3);
    assert_eq!(w.at(0.5), Vec2::new(45.0, 30.0));
}

#[test]
fn hull_bounds_every_control_point() {
    // Hand-computed: the backward-loop wire above spans x from -200
    // (p2) to 600 (p1) and y stays 0, so the hull is 800 wide and flat.
    let w = Wire::data(Vec2::new(400.0, 0.0), Vec2::new(0.0, 0.0));
    let hull = w.hull();
    assert_close(hull.min.x, -200.0);
    assert_close(hull.size.w, 800.0);
    assert_close(hull.size.h, 0.0);

    // A stacked span's hull covers both endpoints' y.
    let w = Wire::data(Vec2::new(0.0, 0.0), Vec2::new(200.0, 600.0));
    let hull = w.hull();
    assert_eq!(hull.min, Vec2::ZERO);
    assert_eq!(hull.size, Size::new(200.0, 600.0));
}

/// The chords run end to end in [`BEZIER_SAMPLES`] steps of `t`: the first leaves `p0`, the last
/// reaches `p3`, and each starts where the one before ended. Over the straight cubic of
/// `x = 0, 16, 32, 48`, the point at `t` is `48 t`, so a chord is 3 long.
#[test]
fn the_chords_walk_the_curve_in_sixteen_steps() {
    let w = Wire {
        p0: Vec2::new(0.0, 0.0),
        p1: Vec2::new(16.0, 0.0),
        p2: Vec2::new(32.0, 0.0),
        p3: Vec2::new(48.0, 0.0),
    };
    let chords: Vec<[Vec2; 2]> = w.chords().collect();
    assert_eq!(chords.len(), BEZIER_SAMPLES);
    assert_eq!(w.chords().len(), BEZIER_SAMPLES);
    assert_eq!(chords[0][0], w.p0);
    assert_eq!(chords[BEZIER_SAMPLES - 1][1], w.p3);
    for (idx, [a, b]) in chords.iter().enumerate() {
        assert_eq!(*a, Vec2::new(3.0 * idx as f32, 0.0), "chord {idx}");
        assert_eq!(*b, Vec2::new(3.0 * (idx + 1) as f32, 0.0), "chord {idx}");
    }
}
