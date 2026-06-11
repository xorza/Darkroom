//! Natural cubic spline numerics: tridiagonal solve for second derivatives plus per-interval
//! evaluation, used to interpolate the tile-grid sky/sigma values C²-continuously.

/// Evaluate natural cubic spline between two nodes.
///
/// Given function values `f0`, `f1` and second derivatives `d0`, `d1` at the
/// endpoints of an interval of width `h`, evaluates the cubic at parameter
/// `t` in [0, 1] (where t=0 gives f0, t=1 gives f1).
///
/// Standard cubic spline formula (Numerical Recipes, SEP/SExtractor):
///   f(t) = (1-t)*f0 + t*f1 + ((1-t)³ - (1-t))*a + (t³ - t)*b
/// where a = h²/6 * d2_0, b = h²/6 * d2_1.
///
/// Factored form (since (ct³-ct) = -t*ct*(2-t) and (t³-t) = -t*ct*(1+t)):
///   f(t) = (1-t)*f0 + t*f1 - t*(1-t)*((2-t)*a + (1+t)*b)
#[inline]
pub(crate) fn cubic_spline_eval(f0: f32, f1: f32, d0: f32, d1: f32, h: f32, t: f32) -> f32 {
    if h <= 0.0 {
        return f0;
    }
    let h2_6 = h * h / 6.0;
    let a = h2_6 * d0;
    let b = h2_6 * d1;
    let ct = 1.0 - t;
    let t_ct = t * ct;
    ct * f0 + t * f1 - t_ct * ((2.0 - t) * a + (1.0 + t) * b)
}

/// Solve for second derivatives of a natural cubic spline.
///
/// Given `n` function values at positions `centers`, computes the second
/// derivatives `d2[0..n]` using a tridiagonal solver with natural boundary
/// conditions (d2[0] = d2[n-1] = 0).
///
/// `scratch` must have length >= `n - 2` (used for modified upper diagonal
/// coefficients in the Thomas algorithm). Pass a reusable buffer to avoid
/// per-call heap allocation.
///
/// Supports non-uniform spacing. O(n) forward elimination + back substitution.
pub(crate) fn solve_natural_spline_d2(
    values: &[f32],
    centers: &[f32],
    d2: &mut [f32],
    scratch: &mut [f32],
) {
    let n = values.len();
    debug_assert_eq!(centers.len(), n);
    debug_assert!(d2.len() >= n);

    if n < 3 {
        // With < 3 points, natural spline has d2 = 0 everywhere
        d2[..n].fill(0.0);
        return;
    }

    // Interval spacings: h[i] = centers[i+1] - centers[i]
    // For n points, we have n-1 intervals and n-2 interior equations.
    //
    // The tridiagonal system for interior points i = 1..n-2:
    //   h[i-1] * d2[i-1] + 2*(h[i-1]+h[i]) * d2[i] + h[i] * d2[i+1]
    //     = 6 * ((f[i+1]-f[i])/h[i] - (f[i]-f[i-1])/h[i-1])
    //
    // With natural BC: d2[0] = 0, d2[n-1] = 0.
    // This reduces to (n-2) equations for d2[1..n-2].

    let m = n - 2; // number of interior unknowns
    debug_assert!(scratch.len() >= m);

    // Forward elimination (Thomas algorithm)
    // We store modified diagonal and RHS in d2[] (reusing output buffer)
    // and use `scratch` for the modified upper diagonal.
    let cp = &mut scratch[..m];

    // First interior equation (i=1):
    let h0 = centers[1] - centers[0];
    let h1 = centers[2] - centers[1];
    let diag = 2.0 * (h0 + h1);
    let rhs = 6.0 * ((values[2] - values[1]) / h1 - (values[1] - values[0]) / h0);

    cp[0] = h1 / diag;
    d2[1] = rhs / diag;

    // Forward sweep for remaining interior equations
    for k in 1..m {
        let i = k + 1; // actual tile index
        let h_prev = centers[i] - centers[i - 1];
        let h_curr = centers[i + 1] - centers[i];
        let d = 2.0 * (h_prev + h_curr);
        let r = 6.0 * ((values[i + 1] - values[i]) / h_curr - (values[i] - values[i - 1]) / h_prev);

        let denom = d - h_prev * cp[k - 1];
        cp[k] = h_curr / denom;
        d2[i] = (r - h_prev * d2[i - 1]) / denom;
    }

    // Back substitution
    for k in (0..m - 1).rev() {
        let i = k + 1;
        d2[i] -= cp[k] * d2[i + 1];
    }

    // Natural boundary conditions
    d2[0] = 0.0;
    d2[n - 1] = 0.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solve_d2_two_points_gives_zero() {
        // Natural spline with 2 points: d2 = 0 everywhere (linear)
        let values = [10.0, 20.0];
        let centers = [0.0, 1.0];
        let mut d2 = [999.0; 2];
        let mut scratch = [0.0; 1];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);
        assert_eq!(d2[0], 0.0);
        assert_eq!(d2[1], 0.0);
    }

    #[test]
    fn test_solve_d2_linear_data_gives_zero() {
        // Linear function f(x) = 2x + 1 at x = 0, 1, 2, 3
        // Second derivative of a linear function is 0 everywhere
        let values = [1.0, 3.0, 5.0, 7.0];
        let centers = [0.0, 1.0, 2.0, 3.0];
        let mut d2 = [0.0; 4];
        let mut scratch = [0.0; 2];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        for (i, &d) in d2.iter().enumerate() {
            assert!(
                d.abs() < 1e-5,
                "d2[{}] = {} should be 0 for linear data",
                i,
                d
            );
        }
    }

    #[test]
    fn test_solve_d2_quadratic_data() {
        // f(x) = x² at x = 0, 1, 2, 3 → f = [0, 1, 4, 9]
        // True second derivative = 2 everywhere
        // Natural spline with n=4 points, uniform h=1:
        //   Interior equations (i=1, i=2):
        //   d2[0]=0, d2[3]=0 (natural BC)
        //
        //   i=1: h0*d2[0] + 2*(h0+h1)*d2[1] + h1*d2[2] = 6*((f2-f1)/h1 - (f1-f0)/h0)
        //        0 + 4*d2[1] + 1*d2[2] = 6*((4-1)/1 - (1-0)/1) = 6*(3-1) = 12
        //
        //   i=2: h1*d2[1] + 2*(h1+h2)*d2[2] + h2*d2[3] = 6*((f3-f2)/h2 - (f2-f1)/h1)
        //        1*d2[1] + 4*d2[2] + 0 = 6*((9-4)/1 - (4-1)/1) = 6*(5-3) = 12
        //
        //   System: 4*d2[1] + d2[2] = 12
        //           d2[1] + 4*d2[2] = 12
        //   Solution: d2[1] = d2[2] = 12/5 = 2.4
        let values = [0.0, 1.0, 4.0, 9.0];
        let centers = [0.0, 1.0, 2.0, 3.0];
        let mut d2 = [0.0; 4];
        let mut scratch = [0.0; 2];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        assert!(d2[0].abs() < 1e-6, "d2[0] = {}, expected 0", d2[0]);
        assert!(d2[3].abs() < 1e-6, "d2[3] = {}, expected 0", d2[3]);
        assert!(
            (d2[1] - 2.4).abs() < 1e-5,
            "d2[1] = {}, expected 2.4",
            d2[1]
        );
        assert!(
            (d2[2] - 2.4).abs() < 1e-5,
            "d2[2] = {}, expected 2.4",
            d2[2]
        );
    }

    #[test]
    fn test_solve_d2_non_uniform_spacing() {
        // f(x) = x² at x = 0, 1, 3 → f = [0, 1, 9]
        // h0 = 1, h1 = 2
        // One interior equation (i=1):
        //   2*(h0+h1)*d2[1] = 6*((f2-f1)/h1 - (f1-f0)/h0)
        //   2*(1+2)*d2[1] = 6*((9-1)/2 - (1-0)/1) = 6*(4-1) = 18
        //   6*d2[1] = 18 → d2[1] = 3
        let values = [0.0, 1.0, 9.0];
        let centers = [0.0, 1.0, 3.0];
        let mut d2 = [0.0; 3];
        let mut scratch = [0.0; 1];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        assert!(d2[0].abs() < 1e-6, "d2[0] = {}, expected 0", d2[0]);
        assert!(d2[2].abs() < 1e-6, "d2[2] = {}, expected 0", d2[2]);
        assert!(
            (d2[1] - 3.0).abs() < 1e-5,
            "d2[1] = {}, expected 3.0",
            d2[1]
        );
    }

    #[test]
    fn test_cubic_spline_eval_endpoints() {
        // At t=0: should return f0; at t=1: should return f1
        let f0 = 10.0;
        let f1 = 20.0;
        let d0 = 5.0;
        let d1 = -3.0;
        let h = 32.0;

        let val_0 = cubic_spline_eval(f0, f1, d0, d1, h, 0.0);
        let val_1 = cubic_spline_eval(f0, f1, d0, d1, h, 1.0);

        assert!(
            (val_0 - f0).abs() < 1e-6,
            "t=0: expected {}, got {}",
            f0,
            val_0
        );
        assert!(
            (val_1 - f1).abs() < 1e-6,
            "t=1: expected {}, got {}",
            f1,
            val_1
        );
    }

    #[test]
    fn test_cubic_spline_eval_midpoint() {
        // At t=0.5, using f(t) = ct*f0 + t*f1 - t*ct*((2-t)*a + (1+t)*b):
        //   = (f0+f1)/2 - 0.375*(a+b)
        // where a = h²/6*d0, b = h²/6*d1
        let f0 = 100.0;
        let f1 = 200.0;
        let h = 6.0; // h²/6 = 6
        let d0 = 2.0; // a = 6*2 = 12
        let d1 = -1.0; // b = 6*(-1) = -6
        // Expected: (100+200)/2 - 0.375*(12 + (-6)) = 150 - 0.375*6 = 150 - 2.25 = 147.75
        let val = cubic_spline_eval(f0, f1, d0, d1, h, 0.5);
        assert!(
            (val - 147.75).abs() < 1e-4,
            "t=0.5: expected 147.75, got {}",
            val
        );
    }

    #[test]
    fn test_cubic_spline_eval_zero_d2_is_linear() {
        // With d0=d1=0, the spline should be exactly linear
        let f0 = 10.0;
        let f1 = 50.0;
        let h = 32.0;

        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let val = cubic_spline_eval(f0, f1, 0.0, 0.0, h, t);
            let expected = (1.0 - t) * f0 + t * f1;
            assert!(
                (val - expected).abs() < 1e-5,
                "t={}: expected {}, got {}",
                t,
                expected,
                val
            );
        }
    }

    #[test]
    fn test_solve_d2_single_point() {
        let values = [42.0];
        let centers = [5.0];
        let mut d2 = [999.0; 1];
        let mut scratch = [0.0; 1];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);
        assert_eq!(d2[0], 0.0);
    }

    #[test]
    fn test_solve_d2_empty() {
        let values: [f32; 0] = [];
        let centers: [f32; 0] = [];
        let mut d2: [f32; 0] = [];
        let mut scratch: [f32; 0] = [];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);
        // No-op, just shouldn't panic
    }

    #[test]
    fn test_solve_d2_five_points_cubic() {
        // f(x) = x³ at x = 0, 1, 2, 3, 4 → f = [0, 1, 8, 27, 64]
        // True f''(x) = 6x, so f''(0)=0, f''(1)=6, f''(2)=12, f''(3)=18, f''(4)=24
        // Natural BC forces d2[0]=0, d2[4]=0, so the spline won't match true f''
        // at interior points. We solve the 3×3 tridiagonal system:
        //
        // h = 1 (uniform spacing)
        // Interior equations (i=1,2,3):
        //   i=1: 4*d2[1] + d2[2] = 6*((f2-f1) - (f1-f0)) = 6*(7 - 1) = 36
        //   i=2: d2[1] + 4*d2[2] + d2[3] = 6*((f3-f2) - (f2-f1)) = 6*(19 - 7) = 72
        //   i=3: d2[2] + 4*d2[3] = 6*((f4-f3) - (f3-f2)) = 6*(37 - 19) = 108
        //
        // Forward elimination:
        //   Row 1: d = 4, cp[0] = 1/4, d2[1] = 36/4 = 9
        //   Row 2: d = 4 - 1*(1/4) = 15/4, cp[1] = 1/(15/4) = 4/15
        //          d2[2] = (72 - 1*9)/(15/4) = 63/(15/4) = 63*4/15 = 252/15 = 16.8
        //   Row 3: d = 4 - 1*(4/15) = 56/15
        //          d2[3] = (108 - 1*16.8)/(56/15) = 91.2/(56/15) = 91.2*15/56 = 1368/56 = 24.4286...
        //
        // Back substitution:
        //   d2[3] = 1368/56 = 24.42857...
        //   d2[2] = 16.8 - (4/15)*24.42857... = 16.8 - 6.51429... = 10.28571...
        //   d2[1] = 9 - (1/4)*10.28571... = 9 - 2.57143... = 6.42857...
        //
        // Exact fractions: d2[1] = 45/7, d2[2] = 72/7, d2[3] = 171/7
        let values = [0.0, 1.0, 8.0, 27.0, 64.0];
        let centers = [0.0, 1.0, 2.0, 3.0, 4.0];
        let mut d2 = [0.0; 5];
        let mut scratch = [0.0; 3];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        assert!(d2[0].abs() < 1e-5, "d2[0] = {}, expected 0", d2[0]);
        assert!(d2[4].abs() < 1e-5, "d2[4] = {}, expected 0", d2[4]);
        let expected_1 = 45.0 / 7.0; // 6.42857...
        let expected_2 = 72.0 / 7.0; // 10.28571...
        let expected_3 = 171.0 / 7.0; // 24.42857...
        assert!(
            (d2[1] - expected_1).abs() < 1e-4,
            "d2[1] = {}, expected {}",
            d2[1],
            expected_1
        );
        assert!(
            (d2[2] - expected_2).abs() < 1e-4,
            "d2[2] = {}, expected {}",
            d2[2],
            expected_2
        );
        assert!(
            (d2[3] - expected_3).abs() < 1e-4,
            "d2[3] = {}, expected {}",
            d2[3],
            expected_3
        );
    }

    #[test]
    fn test_solve_d2_symmetric_data() {
        // f = [1, 4, 9, 4, 1] at x = [0, 1, 2, 3, 4] (symmetric around x=2)
        // Symmetry requires d2[1] == d2[3] and d2[2] is the center value.
        //
        // Interior equations (uniform h=1):
        //   i=1: 4*d2[1] + d2[2] = 6*((9-4) - (4-1)) = 6*(5-3) = 12
        //   i=2: d2[1] + 4*d2[2] + d2[3] = 6*((4-9) - (9-4)) = 6*(-5-5) = -60
        //   i=3: d2[2] + 4*d2[3] = 6*((1-4) - (4-9)) = 6*(-3+5) = 12
        //
        // By symmetry d2[1] = d2[3]. Let a = d2[1] = d2[3], b = d2[2]:
        //   4a + b = 12
        //   a + 4b + a = -60 → 2a + 4b = -60 → a + 2b = -30
        //   From first: b = 12 - 4a
        //   Substitute: a + 2(12-4a) = -30 → a + 24 - 8a = -30 → -7a = -54 → a = 54/7
        //   b = 12 - 4*54/7 = 12 - 216/7 = (84-216)/7 = -132/7
        let values = [1.0, 4.0, 9.0, 4.0, 1.0];
        let centers = [0.0, 1.0, 2.0, 3.0, 4.0];
        let mut d2 = [0.0; 5];
        let mut scratch = [0.0; 3];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        let expected_sym = 54.0 / 7.0; // d2[1] = d2[3]
        let expected_center = -132.0 / 7.0; // d2[2]

        assert!(d2[0].abs() < 1e-5, "d2[0] = {}, expected 0", d2[0]);
        assert!(d2[4].abs() < 1e-5, "d2[4] = {}, expected 0", d2[4]);
        assert!(
            (d2[1] - expected_sym).abs() < 1e-4,
            "d2[1] = {}, expected {} (54/7)",
            d2[1],
            expected_sym
        );
        assert!(
            (d2[3] - expected_sym).abs() < 1e-4,
            "d2[3] = {}, expected {} (54/7)",
            d2[3],
            expected_sym
        );
        assert!(
            (d2[1] - d2[3]).abs() < 1e-6,
            "Symmetry broken: d2[1]={} != d2[3]={}",
            d2[1],
            d2[3]
        );
        assert!(
            (d2[2] - expected_center).abs() < 1e-4,
            "d2[2] = {}, expected {} (-132/7)",
            d2[2],
            expected_center
        );
    }

    #[test]
    fn test_cubic_spline_eval_h_zero_returns_f0() {
        // When h=0 (degenerate interval), should return f0
        let val = cubic_spline_eval(42.0, 99.0, 5.0, -3.0, 0.0, 0.5);
        assert_eq!(val, 42.0);
    }

    #[test]
    fn test_cubic_spline_eval_h_negative_returns_f0() {
        let val = cubic_spline_eval(42.0, 99.0, 5.0, -3.0, -1.0, 0.5);
        assert_eq!(val, 42.0);
    }

    #[test]
    fn test_spline_roundtrip_reproduces_nodes() {
        // Solve d2 for f = [0, 1, 8, 27, 64] (x³), then verify that evaluating
        // the spline at each node point exactly reproduces the function value.
        let values = [0.0f32, 1.0, 8.0, 27.0, 64.0];
        let centers = [0.0f32, 1.0, 2.0, 3.0, 4.0];
        let mut d2 = [0.0f32; 5];
        let mut scratch = [0.0f32; 3];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        // At each node i, evaluate spline from interval [i-1, i] at t=1
        // and from interval [i, i+1] at t=0. Both should give values[i].
        for i in 0..5 {
            // From left interval (t=1): interval [i-1, i]
            if i > 0 {
                let h = centers[i] - centers[i - 1];
                let val = cubic_spline_eval(values[i - 1], values[i], d2[i - 1], d2[i], h, 1.0);
                assert!(
                    (val - values[i]).abs() < 1e-4,
                    "Node {} from left: expected {}, got {}",
                    i,
                    values[i],
                    val
                );
            }
            // From right interval (t=0): interval [i, i+1]
            if i < 4 {
                let h = centers[i + 1] - centers[i];
                let val = cubic_spline_eval(values[i], values[i + 1], d2[i], d2[i + 1], h, 0.0);
                assert!(
                    (val - values[i]).abs() < 1e-4,
                    "Node {} from right: expected {}, got {}",
                    i,
                    values[i],
                    val
                );
            }
        }
    }

    #[test]
    fn test_spline_roundtrip_interior_continuity() {
        // At each interior node, the value from the left interval (t=1) should
        // match the value from the right interval (t=0). This tests C0 continuity.
        // Also test that the first derivative is continuous (C1).
        let values = [2.0f32, 5.0, 3.0, 8.0, 1.0, 6.0];
        let centers = [0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0];
        let mut d2 = [0.0f32; 6];
        let mut scratch = [0.0f32; 4];

        solve_natural_spline_d2(&values, &centers, &mut d2, &mut scratch);

        for i in 1..5 {
            let h_left = centers[i] - centers[i - 1];
            let h_right = centers[i + 1] - centers[i];

            let val_left =
                cubic_spline_eval(values[i - 1], values[i], d2[i - 1], d2[i], h_left, 1.0);
            let val_right =
                cubic_spline_eval(values[i], values[i + 1], d2[i], d2[i + 1], h_right, 0.0);

            assert!(
                (val_left - val_right).abs() < 1e-4,
                "C0 break at node {}: left={}, right={}",
                i,
                val_left,
                val_right
            );

            // C1 check: numerical derivative from both sides should match
            let eps = 1e-4;
            let val_left_m = cubic_spline_eval(
                values[i - 1],
                values[i],
                d2[i - 1],
                d2[i],
                h_left,
                1.0 - eps,
            );
            let val_right_p =
                cubic_spline_eval(values[i], values[i + 1], d2[i], d2[i + 1], h_right, eps);
            // Derivative from left: (val_left - val_left_m) / (eps * h_left)
            // Derivative from right: (val_right_p - val_right) / (eps * h_right)
            let deriv_left = (val_left - val_left_m) / (eps * h_left);
            let deriv_right = (val_right_p - val_right) / (eps * h_right);
            assert!(
                (deriv_left - deriv_right).abs() < 0.1,
                "C1 break at node {}: deriv_left={}, deriv_right={}",
                i,
                deriv_left,
                deriv_right
            );
        }
    }
}
