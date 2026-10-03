use std::f64::consts::PI;

use super::*;

/// `sgarea`: the signed area between a segment and the x axis, inside the unit square — positive
/// left to right. Each row is `(from, to, area, derivation)`. Every area is a sum of products of
/// halves and quarters, exact in f64.
#[test]
fn sgarea_hand_values() {
    for (from, to, expected, derivation) in [
        (
            (0.0, 0.5),
            (1.0, 0.5),
            0.5,
            "horizontal at y ½ across the square: 1·½",
        ),
        (
            (1.0, 0.5),
            (0.0, 0.5),
            -0.5,
            "the same right to left: the sign flips",
        ),
        ((0.5, 0.0), (0.5, 1.0), 0.0, "vertical: no width"),
        (
            (0.5, 0.0),
            (0.5 + 1e-16, 1.0),
            0.0,
            "near vertical: a width under rounding",
        ),
        (
            (0.5, 0.0),
            (0.5 - 1e-16, 1.0),
            0.0,
            "near vertical the other way",
        ),
        ((1.5, 0.0), (2.5, 1.0), 0.0, "right of the square"),
        ((0.0, -1.0), (1.0, -0.5), 0.0, "below the square"),
        (
            (0.0, 1.5),
            (1.0, 2.0),
            1.0,
            "above the square: its whole width, 1·1",
        ),
        (
            (0.0, 0.0),
            (1.0, 1.0),
            0.5,
            "the diagonal: a triangle ½·1·1",
        ),
        (
            (0.0, 0.5),
            (1.0, 1.5),
            0.875,
            "leaves through the top at x ½: trapezoid ½·½·(½ + 1) + the strip 1·½",
        ),
        (
            (0.0, 1.5),
            (1.0, 0.5),
            0.875,
            "enters through the top at x ½: the strip ½·1 + trapezoid ½·½·(1 + ½)",
        ),
        (
            (0.0, -0.5),
            (1.0, 0.5),
            0.125,
            "enters through the bottom at x ½: a triangle ½·½·½",
        ),
    ] {
        let area = sgarea(DVec2::new(from.0, from.1), DVec2::new(to.0, to.1));
        assert_eq!(area, expected, "{derivation}: {from:?} → {to:?}");
    }
}

/// `boxer`: the area a quadrilateral shares with the unit cell whose lower-left corner is given.
/// Each row is `(corner, quad, area, derivation)`; the axis-aligned ones are exact.
#[test]
fn boxer_hand_values() {
    let square = |x: f64, y: f64, side: f64| {
        [
            DVec2::new(x, y),
            DVec2::new(x + side, y),
            DVec2::new(x + side, y + side),
            DVec2::new(x, y + side),
        ]
    };
    for (corner, quad, expected, derivation) in [
        ((0.0, 0.0), square(0.0, 0.0, 1.0), 1.0, "the cell itself"),
        (
            (0.0, 0.0),
            square(0.5, 0.0, 1.0),
            0.5,
            "shifted ½ along x: ½·1",
        ),
        (
            (0.0, 0.0),
            square(0.5, 0.5, 1.0),
            0.25,
            "shifted ½ along both: ½·½",
        ),
        (
            (3.0, 5.0),
            square(3.0, 5.0, 1.0),
            1.0,
            "the cell (3, 5) itself",
        ),
        ((0.0, 0.0), square(5.0, 5.0, 1.0), 0.0, "far from the cell"),
        (
            (0.0, 0.0),
            [
                DVec2::new(0.5, 0.0),
                DVec2::new(1.0, 0.5),
                DVec2::new(0.5, 1.0),
                DVec2::new(0.0, 0.5),
            ],
            0.5,
            "the diamond on the edge midpoints: half the cell",
        ),
    ] {
        let area = boxer(DVec2::new(corner.0, corner.1), &quad);
        assert_eq!(area, expected, "{derivation}");
    }
}

/// A unit square turned 30° about the cell's centre overhangs each side of the cell by a triangle.
/// A corner of the turned square sits `½·(cos 30° + sin 30°) = (√3 + 1)/4` from the centre along an
/// axis, so it pokes `h = (√3 − 1)/4` past the side. The square's edges leave that corner at 30° and
/// 60° to the side, so the triangle's base is `h·(√3 + 1/√3) = 4h/√3` and its area `2h²/√3`. With
/// `h² = (2 − √3)/8` the four triangles take `8h²/√3 = 2/√3 − 1`, and the overlap is `2 − 2/√3`.
/// The clipping is f64 arithmetic on irrational corners: a few ulps of 1.
#[test]
fn boxer_of_a_turned_square_is_the_square_less_four_triangles() {
    let (sin, cos) = (PI / 6.0).sin_cos();
    let quad = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)]
        .map(|(dx, dy)| DVec2::new(0.5 + dx * cos - dy * sin, 0.5 + dx * sin + dy * cos));
    let area = boxer(DVec2::ZERO, &quad);
    let expected = 2.0 - 2.0 / 3f64.sqrt();
    assert!(
        (area - expected).abs() <= 16.0 * f64::EPSILON,
        "{area}, expected {expected}"
    );
}
