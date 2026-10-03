use super::*;

/// A sample is degenerate when two of its points are under 1 px apart — `dist² < 1`, so exactly
/// 1 px is not — or when it has three or more points within a cross product of 1 of the line
/// through the first two.
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
            &[p(0.0, 0.0), p(10.0, 0.0), p(20.0, 0.1)][..],
            true,
            "within a cross product of 1 of the line: 10·0.1 = 1",
        ),
        (
            &[p(0.0, 0.0), p(10.0, 0.0), p(5.0, 10.0)][..],
            false,
            "a triangle: cross product 100",
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
            &[p(0.0, 0.0), p(100.0, 0.0), p(100.0, 100.0), p(0.0, 100.0)][..],
            false,
            "a square",
        ),
    ] {
        assert_eq!(is_sample_degenerate(points), degenerate, "{why}");
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
