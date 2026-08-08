//! Shared test helpers for centroid fitting tests (gaussian_fit, moffat_fit).

use crate::stacking::star_detection::centroid::lm_optimizer::NormalEquations;
use crate::testing::synthetic::patterns::add_gaussian_noise;

/// Add Gaussian noise to pixel values using a simple LCG PRNG.
pub(super) fn add_noise(pixels: &mut [f32], noise_sigma: f32, seed: u64) {
    add_gaussian_noise(pixels, noise_sigma, seed);
}

/// Compare two f64 values with absolute + relative tolerance.
///
/// Uses absolute tolerance 1e-14 for values near zero,
/// and relative tolerance 1e-10 for larger values.
/// Suitable for comparing SIMD vs scalar results where FMA rounding differs.
pub(super) fn approx_eq(a: f64, b: f64) -> bool {
    let abs_diff = (a - b).abs();
    // Absolute tolerance for values near zero
    if abs_diff < 1e-14 {
        return true;
    }
    // Relative tolerance for larger values
    let max_abs = a.abs().max(b.abs());
    abs_diff / max_abs < 1e-10
}

/// Scalar reference for the normal equations: `J^T·J`, `J^T·r`, and `Σr²`, from a jacobian and
/// residuals computed by the caller.
///
/// Ground truth in SIMD-vs-scalar validation tests, so it re-derives the symmetric fill instead
/// of calling [`NormalEquations::mirror_lower_triangle`] — sharing that step with the code under
/// test would let a bug in it pass unnoticed. Unweighted, matching the unweighted batch paths it
/// is compared against.
#[allow(clippy::needless_range_loop)]
pub(super) fn reference_normal_equations<const N: usize>(
    jacobian: &[[f64; N]],
    residuals: &[f64],
) -> NormalEquations<N> {
    let mut equations = NormalEquations::zeroed();
    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        equations.chi2 += r * r;
        for i in 0..N {
            equations.gradient[i] += row[i] * r;
            for j in i..N {
                equations.hessian[i][j] += row[i] * row[j];
            }
        }
    }
    for i in 1..N {
        for j in 0..i {
            equations.hessian[i][j] = equations.hessian[j][i];
        }
    }
    equations
}
