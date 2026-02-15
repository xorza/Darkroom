//! Shared test utilities for centroid fitting tests (gaussian_fit, moffat_fit).

use glam::Vec2;

use crate::common::Buffer2;

/// Add Gaussian noise to pixel values using a simple LCG PRNG.
pub fn add_noise(pixels: &mut [f32], noise_sigma: f32, seed: u64) {
    crate::testing::synthetic::patterns::add_gaussian_noise(pixels, noise_sigma, seed);
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

// ============================================================================
// Stamp generators
// ============================================================================

/// Generate a circular Gaussian star stamp.
pub fn make_gaussian_star(
    width: usize,
    height: usize,
    pos: Vec2,
    sigma: f32,
    amplitude: f32,
    background: f32,
) -> Buffer2<f32> {
    let inv_2sigma2 = 0.5 / (sigma * sigma);
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - pos.x;
            let dy = y as f32 - pos.y;
            pixels[y * width + x] += amplitude * (-(dx * dx + dy * dy) * inv_2sigma2).exp();
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Generate an elliptical Gaussian star stamp.
pub fn make_elliptical_star(
    width: usize,
    height: usize,
    pos: Vec2,
    sigma_x: f32,
    sigma_y: f32,
    amplitude: f32,
    background: f32,
) -> Buffer2<f32> {
    let inv_sx2 = 1.0 / (sigma_x * sigma_x);
    let inv_sy2 = 1.0 / (sigma_y * sigma_y);
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - pos.x;
            let dy = y as f32 - pos.y;
            let exponent = -0.5 * (dx * dx * inv_sx2 + dy * dy * inv_sy2);
            pixels[y * width + x] += amplitude * exponent.exp();
        }
    }
    Buffer2::new(width, height, pixels)
}

/// Generate a Moffat profile star stamp.
pub fn make_moffat_star(
    width: usize,
    height: usize,
    pos: Vec2,
    alpha: f32,
    beta: f32,
    amplitude: f32,
    background: f32,
) -> Buffer2<f32> {
    let inv_alpha2 = 1.0 / (alpha * alpha);
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - pos.x;
            let dy = y as f32 - pos.y;
            let r2 = dx * dx + dy * dy;
            pixels[y * width + x] += amplitude * (1.0 + r2 * inv_alpha2).powf(-beta);
        }
    }
    Buffer2::new(width, height, pixels)
}
