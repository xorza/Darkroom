//! 2D Gaussian fitting for high-precision centroid computation.
//!
//! Implements Levenberg-Marquardt optimization to fit a 2D Gaussian model:
//! f(x,y) = A × exp(-((x-x₀)²/2σ_x² + (y-y₀)²/2σ_y²)) + B
//!
//! Uses f64 throughout the fitting pipeline for numerical stability,
//! achieving ~0.01 pixel centroid accuracy.

#[cfg(all(test, feature = "internals"))]
mod bench;
#[cfg(test)]
mod tests;

/// Coefficients the two SIMD backends' `exp` approximations share.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod exp_poly;

#[cfg(target_arch = "x86_64")]
mod simd_avx2;

#[cfg(target_arch = "aarch64")]
mod simd_neon;

use crate::simd::dispatch;
use crate::stacking::star_detection::centroid::lm_optimizer::{
    FitData, LMConfig, LMModel, NormalEquations, accumulate_chi2, build_normal_equations_scalar,
    optimize,
};
use crate::stacking::star_detection::centroid::{
    FitNoise, MAX_STAMP_PIXELS, estimate_sigma_from_moments, extract_stamp, fit_weights,
};
use arrayvec::ArrayVec;
use glam::Vec2;
use imaginarium::Buffer2;

/// Configuration for Gaussian fitting.
pub(super) type GaussianFitConfig = LMConfig;

/// Result of 2D Gaussian fitting.
#[derive(Debug, Clone, Copy)]
pub(super) struct GaussianFitResult {
    /// Position of Gaussian center (sub-pixel).
    pub(super) pos: Vec2,
    /// Sigma in X and Y directions.
    pub(super) sigma: Vec2,
    /// Whether the fit converged.
    pub(super) converged: bool,
    /// Fit diagnostics that no production caller reads — `measure_star` only uses
    /// `pos`/`sigma`/`converged` — but that tests need to verify LM convergence against
    /// synthetic ground truth. Gated rather than carried and ignored, so a release build
    /// neither stores them nor runs the arithmetic that fills them.
    #[cfg(test)]
    debug: GaussianFitDebug,
}

/// Fit diagnostics kept for tests; see [`GaussianFitResult::debug`].
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct GaussianFitDebug {
    /// Amplitude of Gaussian.
    amplitude: f32,
    /// Background level.
    background: f32,
    /// RMS residual of fit.
    rms_residual: f32,
    /// Number of iterations used.
    iterations: usize,
}

/// 2D Gaussian model for L-M optimization (6 parameters).
/// Parameters: [x0, y0, amplitude, sigma_x, sigma_y, background]
#[derive(Debug)]
struct Gaussian2D {
    stamp_radius: f64,
}

impl LMModel<6> for Gaussian2D {
    #[inline]
    fn evaluate(&self, x: f64, y: f64, params: &[f64; 6]) -> f64 {
        let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / (sigma_x * sigma_x) + dy * dy / (sigma_y * sigma_y));
        amp * exponent.exp() + bg
    }

    #[inline]
    fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; 6]) -> (f64, [f64; 6]) {
        let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
        let sigma_x2 = sigma_x * sigma_x;
        let sigma_y2 = sigma_y * sigma_y;
        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / sigma_x2 + dy * dy / sigma_y2);
        let exp_val = exponent.exp();
        let amp_exp = amp * exp_val;
        let model_val = amp_exp + bg;

        (
            model_val,
            [
                amp_exp * dx / sigma_x2,                  // df/dx0
                amp_exp * dy / sigma_y2,                  // df/dy0
                exp_val,                                  // df/damp
                amp_exp * dx * dx / (sigma_x2 * sigma_x), // df/dsigma_x
                amp_exp * dy * dy / (sigma_y2 * sigma_y), // df/dsigma_y
                1.0,                                      // df/dbg
            ],
        )
    }

    #[inline]
    fn constrain(&self, params: &mut [f64; 6]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Sigma_x
        params[4] = params[4].clamp(0.5, self.stamp_radius); // Sigma_y
    }

    fn batch_build_normal_equations(&self, data: FitData, params: &[f64; 6]) -> NormalEquations<6> {
        // The SIMD kernels are unweighted-only, so a weighted fit takes the scalar path.
        dispatch! {
            x86: avx2_fma if data.weights.is_none() => simd_avx2::batch_build_normal_equations_avx2(
                self, data.x, data.y, data.z, params,
            ),
            aarch64 if data.weights.is_none() => simd_neon::batch_build_normal_equations_neon(
                self, data.x, data.y, data.z, params,
            ),
            scalar => build_normal_equations_scalar(self, data, params),
        }
    }

    fn batch_compute_chi2(&self, data: FitData, params: &[f64; 6]) -> f64 {
        // The SIMD kernels are unweighted-only, so a weighted fit takes the scalar path.
        dispatch! {
            x86: avx2_fma if data.weights.is_none()
                => simd_avx2::batch_compute_chi2_avx2(self, data.x, data.y, data.z, params),
            aarch64 if data.weights.is_none()
                => simd_neon::batch_compute_chi2_neon(self, data.x, data.y, data.z, params),
            scalar => accumulate_chi2(self, data, params, 0..data.len()),
        }
    }
}

/// Fit a 2D Gaussian to a star stamp via Levenberg-Marquardt (f64 throughout, ~0.01 px
/// centroid accuracy). When `noise` is set, each pixel is weighted by `1/σ²` from the CCD
/// noise model so the shot-noisy bright core doesn't bias the fit (PR1); `None` is a plain
/// unweighted fit.
pub(super) fn fit_gaussian_2d(
    pixels: &Buffer2<f32>,
    pos: Vec2,
    stamp_radius: usize,
    background: f32,
    noise: Option<FitNoise>,
    config: &GaussianFitConfig,
) -> Option<GaussianFitResult> {
    let stamp = extract_stamp(pixels, pos, stamp_radius)?;

    let n = stamp.x.len();
    if n < 7 {
        return None;
    }

    // Convert stamp data to f64 for fitting. Stack-allocated (stamp size is bounded
    // by MAX_STAMP_PIXELS), so the parallel per-star fit loop makes no heap allocations.
    let data_x: ArrayVec<f64, MAX_STAMP_PIXELS> = stamp.x.iter().map(|&v| v as f64).collect();
    let data_y: ArrayVec<f64, MAX_STAMP_PIXELS> = stamp.y.iter().map(|&v| v as f64).collect();
    let data_z: ArrayVec<f64, MAX_STAMP_PIXELS> = stamp.z.iter().map(|&v| v as f64).collect();

    let weights = fit_weights(&data_z, background, noise);

    // Estimate sigma from moments for better initial guess
    let sigma_est = estimate_sigma_from_moments(&stamp.x, &stamp.y, &stamp.z, pos, background);

    let initial_params: [f64; 6] = [
        pos.x as f64,
        pos.y as f64,
        (stamp.peak - background).max(0.01) as f64,
        sigma_est as f64,
        sigma_est as f64,
        background as f64,
    ];

    let model = Gaussian2D {
        stamp_radius: stamp_radius as f64,
    };

    let data = FitData::new(&data_x, &data_y, &data_z, weights.as_deref());
    let result = optimize(&model, data, initial_params, config);

    let [x0, y0, _, sigma_x, sigma_y, _] = result.params;
    let result_pos = Vec2::new(x0 as f32, y0 as f32);

    if !validate_fit(result_pos, pos, sigma_x, sigma_y, stamp_radius) {
        return None;
    }

    Some(GaussianFitResult {
        pos: result_pos,
        sigma: Vec2::new(sigma_x as f32, sigma_y as f32),
        converged: result.converged,
        #[cfg(test)]
        debug: GaussianFitDebug::of(&result, n),
    })
}

/// Centre inside the stamp, and both sigmas within a plausible range.
///
/// The sigma bounds are phrased as acceptance rather than rejection, which is what makes a
/// non-finite one fail: comparisons against NaN are all false, so `NaN > limit` reads as "not out
/// of range" and a rejection-phrased check would pass a NaN sigma through to `Star::fwhm`.
///
/// The centre needs its own [`Vec2::is_finite`] check, because that trick does not extend to it:
/// `max_element` reduces with [`f32::max`], which *ignores* NaN and returns the other lane, so a
/// NaN x-coordinate would silently compare as the (finite) y-offset.
///
/// A rejected fit is not an error — `measure_star` falls back to the moment-based centroid.
fn validate_fit(
    result_pos: Vec2,
    input_pos: Vec2,
    sigma_x: f64,
    sigma_y: f64,
    stamp_radius: usize,
) -> bool {
    let plausible_sigma = 0.5..=stamp_radius as f64 * 2.0;
    result_pos.is_finite()
        && (result_pos - input_pos).abs().max_element() <= stamp_radius as f32
        && plausible_sigma.contains(&sigma_x)
        && plausible_sigma.contains(&sigma_y)
}

#[cfg(test)]
mod internals {
    use crate::stacking::star_detection::centroid::gaussian_fit::{Gaussian2D, GaussianFitDebug};
    use crate::stacking::star_detection::centroid::lm_optimizer::LMResult;

    impl GaussianFitDebug {
        /// Derive the diagnostics from the optimizer's report, where `n` is the sample count the
        /// χ² was summed over. Gated with the struct, so a release build runs none of this.
        pub(super) fn of(result: &LMResult<6>, n: usize) -> Self {
            let [_, _, amplitude, _, _, background] = result.params;
            Self {
                amplitude: amplitude as f32,
                background: background as f32,
                rms_residual: (result.chi2 / n as f64).sqrt() as f32,
                iterations: result.iterations,
            }
        }
    }

    impl Gaussian2D {
        /// The Jacobian row alone, derived independently of
        /// [`Gaussian2D::evaluate_and_jacobian`]'s fused form.
        ///
        /// Production takes only the fused path; this exists so
        /// `test_gaussian_evaluate_and_jacobian_consistency` has a second derivation of the same
        /// algebra to check it against. Keep the two written out separately — sharing a helper
        /// between them would make the test compare an expression with itself.
        pub(super) fn jacobian_row(&self, x: f64, y: f64, params: &[f64; 6]) -> [f64; 6] {
            let [x0, y0, amp, sigma_x, sigma_y, _bg] = *params;
            let sigma_x2 = sigma_x * sigma_x;
            let sigma_y2 = sigma_y * sigma_y;
            let dx = x - x0;
            let dy = y - y0;
            let exponent = -0.5 * (dx * dx / sigma_x2 + dy * dy / sigma_y2);
            let exp_val = exponent.exp();
            let amp_exp = amp * exp_val;

            [
                amp_exp * dx / sigma_x2,                  // df/dx0
                amp_exp * dy / sigma_y2,                  // df/dy0
                exp_val,                                  // df/damp
                amp_exp * dx * dx / (sigma_x2 * sigma_x), // df/dsigma_x
                amp_exp * dy * dy / (sigma_y2 * sigma_y), // df/dsigma_y
                1.0,                                      // df/dbg
            ]
        }
    }
}
