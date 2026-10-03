use crate::background_mesh::spline::spline_segment::SplineSegment;
use crate::background_mesh::spline::*;

/// One natural-spline system, hand-solved.
#[derive(Debug)]
struct SplineCase {
    name: &'static str,
    values: &'static [f32],
    centers: &'static [f32],
    d2: &'static [f64],
}

/// The interior equation at node `i` of a natural spline through `values` at `centers`:
/// `h₀·d″ᵢ₋₁ + 2(h₀ + h₁)·d″ᵢ + h₁·d″ᵢ₊₁ = 6·((fᵢ₊₁ − fᵢ)/h₁ − (fᵢ − fᵢ₋₁)/h₀)`, and the two
/// segments' slopes at the node, `(f − f₋)/h₀ + h₀(d″₋ + 2d″)/6` from the left and
/// `(f₊ − f)/h₁ − h₁(2d″ + d″₊)/6` from the right, meet exactly when it holds.
fn slope_mismatch(values: &[f32], centers: &[f32], d2: &[f32], i: usize) -> f64 {
    let at = |k: usize| {
        (
            f64::from(values[k]),
            f64::from(centers[k]),
            f64::from(d2[k]),
        )
    };
    let ((f_l, x_l, d_l), (f, x, d), (f_r, x_r, d_r)) = (at(i - 1), at(i), at(i + 1));
    let (h_l, h_r) = (x - x_l, x_r - x);
    let left = (f - f_l) / h_l + h_l * (d_l + 2.0 * d) / 6.0;
    let right = (f_r - f) / h_r - h_r * (2.0 * d + d_r) / 6.0;
    left - right
}

/// The second derivatives `solve_natural_spline_d2` finds, against systems solved by hand. Under
/// three nodes there are no interior equations and every `d″` is 0; linear data has a zero
/// right-hand side. With `h = 1` the interior rows read `d″₋ + 4d″ + d″₊ = 6·Δ²f`:
/// - x² at 0..3: `4a + b = 12`, `a + 4b = 12`, so `a = b = 12/5`;
/// - x³ at 0..4: right-hand sides 36, 72, 108, solved to 45/7, 72/7, 171/7;
/// - [1, 4, 9, 4, 1]: 12, −60, 12, symmetric, so `4a + b = 12`, `a + 2b = −30`: 54/7 and −132/7.
///
/// Non-uniform, x² at 0, 1, 3 (`h` = 1, 2): `6·d″ = 6·(8/2 − 1/1)`, so 3. And x² at 0, 2, 4, 5 —
/// a short last interval, as a frame whose width is not a whole number of tiles leaves:
/// `8a + 2b = 6·(12/2 − 4/2) = 24`, `2a + 6b = 6·(9/1 − 12/2) = 18`, so `a = 27/11`, `b = 24/11`.
/// That case runs both the forward sweep and the back substitution once each.
///
/// The sweep rounds each unknown about four times and the back substitution twice more, and the
/// system's diagonal dominance keeps those from growing: 8ε of the largest `d″` bounds it.
#[test]
fn solve_d2_hand_solved() {
    let cases = [
        SplineCase {
            name: "empty",
            values: &[],
            centers: &[],
            d2: &[],
        },
        SplineCase {
            name: "one node",
            values: &[42.0],
            centers: &[5.0],
            d2: &[0.0],
        },
        SplineCase {
            name: "two nodes",
            values: &[10.0, 20.0],
            centers: &[0.0, 1.0],
            d2: &[0.0, 0.0],
        },
        SplineCase {
            name: "linear",
            values: &[1.0, 3.0, 5.0, 7.0],
            centers: &[0.0, 1.0, 2.0, 3.0],
            d2: &[0.0; 4],
        },
        SplineCase {
            name: "quadratic",
            values: &[0.0, 1.0, 4.0, 9.0],
            centers: &[0.0, 1.0, 2.0, 3.0],
            d2: &[0.0, 2.4, 2.4, 0.0],
        },
        SplineCase {
            name: "cubic",
            values: &[0.0, 1.0, 8.0, 27.0, 64.0],
            centers: &[0.0, 1.0, 2.0, 3.0, 4.0],
            d2: &[0.0, 45.0 / 7.0, 72.0 / 7.0, 171.0 / 7.0, 0.0],
        },
        SplineCase {
            name: "symmetric",
            values: &[1.0, 4.0, 9.0, 4.0, 1.0],
            centers: &[0.0, 1.0, 2.0, 3.0, 4.0],
            d2: &[0.0, 54.0 / 7.0, -132.0 / 7.0, 54.0 / 7.0, 0.0],
        },
        SplineCase {
            name: "non-uniform, three nodes",
            values: &[0.0, 1.0, 9.0],
            centers: &[0.0, 1.0, 3.0],
            d2: &[0.0, 3.0, 0.0],
        },
        SplineCase {
            name: "non-uniform, short last interval",
            values: &[0.0, 4.0, 16.0, 25.0],
            centers: &[0.0, 2.0, 4.0, 5.0],
            d2: &[0.0, 27.0 / 11.0, 24.0 / 11.0, 0.0],
        },
    ];
    for case in cases {
        let n = case.values.len();
        let mut d2 = vec![999.0f32; n];
        let mut scratch = vec![0.0f32; n.saturating_sub(2).max(1)];
        solve_natural_spline_d2(case.values, case.centers, &mut d2, &mut scratch);
        let largest = case.d2.iter().fold(0.0f64, |m, d| m.max(d.abs()));
        let bound = 8.0 * f64::from(f32::EPSILON) * largest;
        for (k, (&got, &expected)) in d2.iter().zip(case.d2).enumerate() {
            assert!(
                (f64::from(got) - expected).abs() <= bound,
                "{}: d″[{k}] = {got}, expected {expected}",
                case.name
            );
        }
    }
}

/// The spline is C¹: at every interior node of irregular data on irregular spacing the two
/// segments' slopes meet. Each `d″` is within 8ε of the largest of them (see
/// [`solve_d2_hand_solved`]), and the slopes weigh the three at a node by `h₀/2` and `h₁/2` in all,
/// so they meet to `(h₀ + h₁)/2 · 8ε · max|d″|`; the slopes themselves are taken in f64.
#[test]
fn spline_slopes_meet_at_every_interior_node() {
    let values = [2.0f32, 5.0, 3.0, 8.0, 1.0, 6.0];
    let centers = [0.0f32, 1.0, 3.0, 4.0, 7.0, 8.0];
    let mut d2 = [0.0f32; 6];
    let mut scratch = [0.0f32; 4];
    solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);
    let largest = d2.iter().fold(0.0f64, |m, &d| m.max(f64::from(d).abs()));
    for i in 1..5 {
        let span = f64::from(centers[i + 1] - centers[i - 1]);
        let bound = span / 2.0 * 8.0 * f64::from(f32::EPSILON) * largest;
        let mismatch = slope_mismatch(&values, &centers, &d2, i);
        assert!(
            mismatch.abs() <= bound,
            "node {i}: {mismatch:e} > {bound:e}"
        );
    }
}

/// A segment at its nodes is its node values; with no curvature it is the straight line between
/// them, exact at dyadic `t`; and at `t = ½` with `h = 6`, `a = 6·2 = 12` and `b = 6·(−1) = −6`,
/// `150 − ¼·(1.5·12 + 1.5·(−6)) = 147.75`. Every step is exact in f32. A zero or negative width is
/// the constant `f0`.
#[test]
fn spline_segment_hand_computed() {
    let curved = SplineSegment::new(10.0, 20.0, 5.0, -3.0, 32.0);
    assert_eq!((curved.eval(0.0), curved.eval(1.0)), (10.0, 20.0));

    let straight = SplineSegment::new(10.0, 50.0, 0.0, 0.0, 32.0);
    for i in 0..=8 {
        let t = i as f32 / 8.0;
        assert_eq!(straight.eval(t), 10.0 + 40.0 * t, "t = {t}");
    }

    assert_eq!(
        SplineSegment::new(100.0, 200.0, 2.0, -1.0, 6.0).eval(0.5),
        147.75
    );

    for h in [0.0, -1.0] {
        assert_eq!(SplineSegment::new(42.0, 99.0, 5.0, -3.0, h).eval(0.5), 42.0);
    }
}
