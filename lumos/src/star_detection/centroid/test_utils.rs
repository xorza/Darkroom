//! Shared test utilities for centroid fitting tests (gaussian_fit, moffat_fit).

/// Add Gaussian noise to pixel values using a simple LCG PRNG.
pub fn add_noise(pixels: &mut [f32], noise_sigma: f32, seed: u64) {
    let mut state = seed;
    for pixel in pixels.iter_mut() {
        // Box-Muller transform with LCG
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u1 = (state as f32) / (u64::MAX as f32);
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u2 = (state as f32) / (u64::MAX as f32);

        let z = (-2.0 * u1.max(1e-10).ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
        *pixel += z * noise_sigma;
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
