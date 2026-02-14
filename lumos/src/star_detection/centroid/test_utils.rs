//! Shared test utilities for centroid fitting tests (gaussian_fit, moffat_fit).

/// Add Gaussian noise to pixel values using a simple LCG PRNG.
pub fn add_noise(pixels: &mut [f32], noise_sigma: f32, seed: u64) {
    let mut rng = crate::testing::TestRng::new(seed);
    for pixel in pixels.iter_mut() {
        *pixel += rng.next_gaussian_f32() * noise_sigma;
    }
}

/// Compare two f64 values with absolute + relative tolerance.
///
/// Uses absolute tolerance 1e-14 for values near zero,
/// and relative tolerance 1e-10 for larger values.
/// Suitable for comparing SIMD vs scalar results where FMA rounding differs.
pub fn approx_eq(a: f64, b: f64) -> bool {
    let abs_diff = (a - b).abs();
    // Absolute tolerance for values near zero
    if abs_diff < 1e-14 {
        return true;
    }
    // Relative tolerance for larger values
    let max_abs = a.abs().max(b.abs());
    abs_diff / max_abs < 1e-10
}

/// Scalar reference for computing J^T J (hessian) and J^T r (gradient).
///
/// Used as ground truth in SIMD-vs-scalar validation tests.
#[allow(clippy::needless_range_loop)]
pub fn compute_hessian_gradient<const N: usize>(
    jacobian: &[[f64; N]],
    residuals: &[f64],
) -> ([[f64; N]; N], [f64; N]) {
    let mut hessian = [[0.0f64; N]; N];
    let mut gradient = [0.0f64; N];
    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..N {
            gradient[i] += row[i] * r;
            for j in i..N {
                hessian[i][j] += row[i] * row[j];
            }
        }
    }
    for i in 1..N {
        for j in 0..i {
            hessian[i][j] = hessian[j][i];
        }
    }
    (hessian, gradient)
}
