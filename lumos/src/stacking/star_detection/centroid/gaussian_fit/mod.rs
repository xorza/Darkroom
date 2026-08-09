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

mod simd;

use crate::stacking::star_detection::centroid::StampGrid;
use crate::stacking::star_detection::centroid::lm_optimizer::{
    FitData, LMConfig, LMModel, NormalEquations, accumulate_chi2, build_normal_equations_scalar,
    optimize,
};
use crate::stacking::star_detection::centroid::{FitNoise, StampFit};
use glam::{DVec2, Vec2};
use imaginarium::Buffer2;

/// Configuration for Gaussian fitting.
pub(super) type GaussianFitConfig = LMConfig;

/// A converged-or-not 2D Gaussian fitted to one star stamp: where its centre landed and
/// how wide it came out. Build one with [`GaussianFit::new`].
#[derive(Debug, Clone, Copy)]
pub(super) struct GaussianFit {
    /// Position of Gaussian center (sub-pixel).
    pub(super) pos: DVec2,
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

/// Fit diagnostics kept for tests; see [`GaussianFit::debug`].
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
        simd::batch_build_normal_equations(self, data, params)
            .unwrap_or_else(|| build_normal_equations_scalar(self, data, params))
    }

    fn batch_compute_chi2(&self, data: FitData, params: &[f64; 6]) -> f64 {
        simd::batch_compute_chi2(self, data, params)
            .unwrap_or_else(|| accumulate_chi2(self, data, params, 0..data.len()))
    }
}

impl GaussianFit {
    /// Fit a 2D Gaussian to a star stamp via Levenberg-Marquardt (f64 throughout, ~0.01 px
    /// centroid accuracy). When `noise` is set, each pixel is weighted by `1/σ²` from the CCD
    /// noise model so the shot-noisy bright core doesn't bias the fit (PR1); `None` is a plain
    /// unweighted fit.
    ///
    /// `None` also when the stamp falls outside the frame, holds too few pixels to constrain six
    /// parameters, or the fit lands somewhere [`validate_fit`] rejects.
    pub(super) fn new(
        pixels: &Buffer2<f32>,
        pos: DVec2,
        grid: &StampGrid,
        background: f32,
        noise: Option<FitNoise>,
        config: &GaussianFitConfig,
    ) -> Option<Self> {
        let fit = StampFit::prepare::<6>(pixels, pos, grid, background, noise)?;

        // Both axes start from the same circular seed; the fit pulls them apart.
        let sigma_est = fit.sigma_est as f64;
        let initial_params: [f64; 6] = [
            fit.local_pos.x,
            fit.local_pos.y,
            fit.amplitude_seed(background),
            sigma_est,
            sigma_est,
            background as f64,
        ];

        let model = Gaussian2D {
            stamp_radius: grid.radius as f64,
        };
        let result = optimize(&model, fit.data(grid), initial_params, config);

        let [x0, y0, _, sigma_x, sigma_y, _] = result.params;
        let result_pos = fit.to_image(x0, y0);

        if !validate_fit(result_pos, pos, sigma_x, sigma_y, grid.radius) {
            return None;
        }

        Some(Self {
            pos: result_pos,
            sigma: Vec2::new(sigma_x as f32, sigma_y as f32),
            converged: result.converged,
            #[cfg(test)]
            debug: GaussianFitDebug::of(&result, fit.stamp.z.len()),
        })
    }
}

/// Centre inside the stamp, and both sigmas within a plausible range.
///
/// The sigma bounds are phrased as acceptance rather than rejection, which is what makes a
/// non-finite one fail: comparisons against NaN are all false, so `NaN > limit` reads as "not out
/// of range" and a rejection-phrased check would pass a NaN sigma through to `Star::fwhm`.
///
/// The centre needs its own [`DVec2::is_finite`] check, because that trick does not extend to it:
/// `max_element` reduces with [`f64::max`], which *ignores* NaN and returns the other lane, so a
/// NaN x-coordinate would silently compare as the (finite) y-offset.
///
/// A rejected fit is not an error — `measure_star` falls back to the moment-based centroid.
fn validate_fit(
    result_pos: DVec2,
    input_pos: DVec2,
    sigma_x: f64,
    sigma_y: f64,
    stamp_radius: usize,
) -> bool {
    let plausible_sigma = 0.5..=stamp_radius as f64 * 2.0;
    result_pos.is_finite()
        && (result_pos - input_pos).abs().max_element() <= stamp_radius as f64
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
