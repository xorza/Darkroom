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
/// conditions (`d2[0] = d2[n-1] = 0`).
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
mod tests;
