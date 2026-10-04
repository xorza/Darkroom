use super::*;
use crate::registration::ransac::transforms::adaptive_iterations;

/// A sample is degenerate at a noise scale of 1 px when two of its points are under 1 px apart —
/// exactly 1 px is not — or when any three of them form a triangle less than 1 px high over its
/// longest side. The height is what the noise blurs, whatever the triangle's size: 0.5 px off a
/// 1000 px baseline is as degenerate as 0.5 px off a 10 px one, and three collinear points that
/// leave out the first are caught too.
#[test]
fn degenerate_samples() {
    let p = DVec2::new;
    for (points, degenerate, why) in [
        (&[][..], false, "empty"),
        (&[p(1.0, 2.0)][..], false, "one point"),
        (&[p(5.0, 5.0), p(5.0, 5.5)][..], true, "0.5 px apart"),
        (&[p(0.0, 0.0), p(0.99, 0.0)][..], true, "0.99 px apart"),
        (&[p(0.0, 0.0), p(1.0, 0.0)][..], false, "exactly 1 px apart"),
        (&[p(0.0, 0.0), p(10.0, 0.0)][..], false, "10 px apart"),
        (
            &[p(0.0, 0.0), p(10.0, 0.0), p(20.0, 0.0)][..],
            true,
            "collinear on x",
        ),
        (
            &[p(0.0, 0.0), p(10.0, 10.0), p(20.0, 20.0)][..],
            true,
            "collinear on y = x",
        ),
        (
            &[p(0.0, 0.0), p(10.0, 0.0), p(5.0, 0.5)][..],
            true,
            "0.5 px high over a 10 px side",
        ),
        (
            &[p(0.0, 0.0), p(1000.0, 0.0), p(500.0, 0.5)][..],
            true,
            "0.5 px high over a 1000 px side",
        ),
        (
            &[p(0.0, 0.0), p(1000.0, 0.0), p(500.0, 2.0)][..],
            false,
            "2 px high over a 1000 px side",
        ),
        (
            &[p(0.0, 0.0), p(10.0, 0.0), p(5.0, 10.0)][..],
            false,
            "a triangle 10 px high",
        ),
        (
            &[p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.1, 0.1)][..],
            true,
            "a quad with a point 0.14 px from another",
        ),
        (
            &[p(0.0, 0.0), p(5.0, 0.0), p(10.0, 0.0), p(15.0, 0.0)][..],
            true,
            "a collinear quad",
        ),
        (
            &[p(0.0, 50.0), p(0.0, 0.0), p(50.0, 0.0), p(100.0, 0.0)][..],
            true,
            "a quad whose last three are collinear",
        ),
        (
            &[p(0.0, 0.0), p(100.0, 0.0), p(100.0, 100.0), p(0.0, 100.0)][..],
            false,
            "a square",
        ),
    ] {
        assert_eq!(is_sample_degenerate(points, 1.0), degenerate, "{why}");
    }
}

/// `N = ⌈ln(1 − p) / ln(1 − wⁿ)⌉`, and 1 at `w` = 0 or 1, where it is undefined.
#[test]
fn adaptive_iterations_hand_values() {
    for (w, n, p, expected, derivation) in [
        (0.0, 2, 0.99, 1, "no inliers"),
        (1.0, 2, 0.99, 1, "all inliers"),
        (0.9, 2, 0.99, 3, "ln 0.01 / ln 0.19 = 2.773"),
        (0.3, 2, 0.99, 49, "ln 0.01 / ln 0.91 = 48.83"),
        (0.5, 2, 0.999, 25, "ln 0.001 / ln 0.75 = 24.01"),
        (0.9, 2, 0.999, 5, "ln 0.001 / ln 0.19 = 4.16"),
        (0.1, 2, 0.999, 688, "ln 0.001 / ln 0.99 = 687.3"),
        (0.5, 3, 0.999, 52, "ln 0.001 / ln 0.875 = 51.73"),
        (0.5, 4, 0.999, 108, "ln 0.001 / ln 0.9375 = 107.03"),
    ] {
        assert_eq!(adaptive_iterations(w, n, p), expected, "{derivation}");
    }
}
