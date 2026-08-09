//! Shared test helpers for centroid fitting tests (gaussian_fit, moffat_fit).

use imaginarium::Buffer2;

use crate::stacking::star_detection::centroid::lm_optimizer::NormalEquations;
use crate::testing::synthetic::patterns::add_gaussian_noise;

/// What gets added to the rendered stamp before fitting.
#[derive(Debug)]
pub(super) enum Perturbation {
    None,
    /// Index-based sawtooth — deterministic without an RNG, and correlated with pixel order
    /// rather than random, which is a different stress than [`Perturbation::Gaussian`].
    Sawtooth {
        amplitude: f32,
    },
    Gaussian {
        sigma: f32,
        seed: u64,
    },
}

impl Perturbation {
    pub(super) fn apply(&self, pixels: &mut Buffer2<f32>) {
        match *self {
            Perturbation::None => {}
            Perturbation::Sawtooth { amplitude } => {
                for (i, p) in pixels.iter_mut().enumerate() {
                    *p += amplitude * ((i % 7) as f32 - 3.0) / 3.0;
                }
            }
            Perturbation::Gaussian { sigma, seed } => add_noise(pixels, sigma, seed),
        }
    }
}

/// Add Gaussian noise to pixel values using a simple LCG PRNG.
pub(super) fn add_noise(pixels: &mut [f32], noise_sigma: f32, seed: u64) {
    add_gaussian_noise(pixels, noise_sigma, seed);
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
