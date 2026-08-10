//! 2D Moffat profile fitting for high-precision centroid computation.
//!
//! The Moffat profile is a better model for stellar PSFs than Gaussian because
//! it has extended wings that match atmospheric seeing:
//!
//! f(x,y) = A × (1 + ((x-x₀)²+(y-y₀)²)/α²)^(-β) + B
//!
//! where α is the core width and β controls the wing slope (typically 2.5-4.5).
//!
//! Uses f64 throughout the fitting pipeline for numerical stability,
//! achieving ~0.01 pixel centroid accuracy.

#[cfg(all(test, feature = "internals"))]
mod bench;
#[cfg(test)]
mod tests;

mod simd;

use crate::math::fwhm::FWHM_TO_SIGMA;
use crate::stacking::star_detection::centroid::fit_is_plausible;
use crate::stacking::star_detection::centroid::lm_optimizer::{
    FitData, LMConfig, LMModel, NormalEquations,
};
use crate::stacking::star_detection::centroid::stamp::FitNoise;
use crate::stacking::star_detection::centroid::stamp::StampFit;
use crate::stacking::star_detection::centroid::stamp::StampGrid;
use glam::DVec2;
use imaginarium::Buffer2;

/// Configuration for Moffat profile fitting.
#[derive(Debug, Clone)]
pub(super) struct MoffatFitConfig {
    /// L-M optimization parameters.
    pub(super) lm: LMConfig,
    /// Fixed Moffat β (wing-slope) used for the fit.
    pub(super) fixed_beta: f32,
}

impl Default for MoffatFitConfig {
    fn default() -> Self {
        Self {
            lm: LMConfig::default(),
            fixed_beta: 2.5,
        }
    }
}

/// A converged-or-not 2D Moffat profile fitted to one star stamp: where its centre landed and
/// the FWHM that follows from its alpha and beta. Build one with [`MoffatFit::new`].
#[derive(Debug, Clone, Copy)]
pub(super) struct MoffatFit {
    /// Position of profile center (sub-pixel).
    pub(super) pos: DVec2,
    /// FWHM computed from alpha and beta.
    pub(super) fwhm: f32,
    /// Whether the fit converged.
    pub(super) converged: bool,
    /// Fit diagnostics that no production caller reads — `measure_star` only uses
    /// `pos`/`fwhm`/`converged` — but that tests need to verify LM convergence against
    /// synthetic ground truth. Gated rather than carried and ignored, so a release build
    /// neither stores them nor runs the arithmetic that fills them.
    #[cfg(test)]
    debug: MoffatFitDebug,
}

/// Fit diagnostics kept for tests; see [`MoffatFit::debug`].
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct MoffatFitDebug {
    /// Amplitude of profile.
    amplitude: f32,
    /// Core width parameter (alpha).
    alpha: f32,
    /// Background level.
    background: f32,
    /// Number of iterations used.
    iterations: usize,
}

/// Strategy for computing `u^(-beta)` efficiently.
/// Pre-computed at model construction to avoid per-pixel branching.
#[derive(Debug, Clone, Copy)]
enum PowStrategy {
    /// beta is a half-integer (n + 0.5): use `1 / (u^n * sqrt(u))`
    HalfInt { int_part: u32 },
    /// beta is an integer: use `1 / u^n`
    Int { n: u32 },
    /// General case: use `u.powf(-beta)`
    General { neg_beta: f64 },
}

/// Compute u^(-beta) using the pre-selected strategy.
#[inline]
fn fast_pow_neg(u: f64, strategy: PowStrategy) -> f64 {
    match strategy {
        PowStrategy::HalfInt { int_part } => {
            // u^(-(n+0.5)) = 1 / (u^n * sqrt(u))
            let u_n = int_pow(u, int_part);
            1.0 / (u_n * u.sqrt())
        }
        PowStrategy::Int { n } => 1.0 / int_pow(u, n),
        PowStrategy::General { neg_beta } => u.powf(neg_beta),
    }
}

/// Compute u^n for small integer n using repeated squaring.
#[inline]
fn int_pow(u: f64, n: u32) -> f64 {
    match n {
        0 => 1.0,
        1 => u,
        2 => u * u,
        3 => u * u * u,
        4 => {
            let u2 = u * u;
            u2 * u2
        }
        5 => {
            let u2 = u * u;
            u2 * u2 * u
        }
        _ => u.powi(n as i32),
    }
}

/// Select optimal strategy for computing u^(-beta).
fn select_pow_strategy(beta: f64) -> PowStrategy {
    let rounded = (beta * 2.0).round();
    let is_half_int = (beta * 2.0 - rounded).abs() < 1e-10;

    if is_half_int {
        let doubled = rounded as i64;
        if doubled % 2 == 0 {
            // Integer beta
            PowStrategy::Int {
                n: (doubled / 2) as u32,
            }
        } else {
            // Half-integer beta (n + 0.5)
            PowStrategy::HalfInt {
                int_part: (doubled / 2) as u32,
            }
        }
    } else {
        PowStrategy::General { neg_beta: -beta }
    }
}

/// Moffat model with fixed beta (5 parameters).
/// Parameters: [x0, y0, amplitude, alpha, background]
#[derive(Debug)]
struct MoffatFixedBeta {
    stamp_radius: f64,
    beta: f64,
    pow_strategy: PowStrategy,
}

impl MoffatFixedBeta {
    fn new(stamp_radius: f64, beta: f64) -> Self {
        Self {
            stamp_radius,
            beta,
            pow_strategy: select_pow_strategy(beta),
        }
    }
}

impl LMModel<5> for MoffatFixedBeta {
    #[inline]
    fn evaluate(&self, x: f64, y: f64, params: &[f64; 5]) -> f64 {
        let [x0, y0, amp, alpha, bg] = *params;
        let r2 = (x - x0).powi(2) + (y - y0).powi(2);
        let u = 1.0 + r2 / (alpha * alpha);
        amp * fast_pow_neg(u, self.pow_strategy) + bg
    }

    #[inline]
    fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; 5]) -> (f64, [f64; 5]) {
        let [x0, y0, amp, alpha, bg] = *params;
        let alpha2 = alpha * alpha;
        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let u_neg_beta = fast_pow_neg(u, self.pow_strategy);
        let model_val = amp * u_neg_beta + bg;
        let u_neg_beta_m1 = u_neg_beta / u;
        let common = 2.0 * amp * self.beta / alpha2 * u_neg_beta_m1;

        (
            model_val,
            [
                common * dx,         // df/dx0
                common * dy,         // df/dy0
                u_neg_beta,          // df/damp
                common * r2 / alpha, // df/dalpha
                1.0,                 // df/dbg
            ],
        )
    }

    #[inline]
    fn constrain(&self, params: &mut [f64; 5]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Alpha
    }

    fn batch_build_normal_equations(&self, data: FitData, params: &[f64; 5]) -> NormalEquations<5> {
        simd::batch_build_normal_equations(self, data, params)
            .unwrap_or_else(|| NormalEquations::from_scalar_pass(self, data, params))
    }

    fn batch_compute_chi2(&self, data: FitData, params: &[f64; 5]) -> f64 {
        simd::batch_compute_chi2(self, data, params)
            .unwrap_or_else(|| self.accumulate_chi2(data, params, 0..data.len()))
    }
}

impl MoffatFit {
    /// Fit a 2D Moffat profile to a star stamp via Levenberg-Marquardt (f64 throughout). When
    /// `noise` is set, each pixel is weighted by `1/σ²` from the CCD noise model so the
    /// shot-noisy bright core doesn't bias the fit (PR1); `None` is a plain unweighted fit.
    ///
    /// `None` also when the stamp falls outside the frame, holds too few pixels to constrain five
    /// parameters, or the fit lands somewhere [`fit_is_plausible`] rejects.
    pub(super) fn new(
        pixels: &Buffer2<f32>,
        pos: DVec2,
        grid: &StampGrid,
        background: f32,
        noise: Option<FitNoise>,
        config: &MoffatFitConfig,
    ) -> Option<Self> {
        // Fixed-β Moffat fits 5 parameters [x0, y0, amplitude, alpha, background].
        let fit = StampFit::prepare::<5>(pixels, pos, grid, background, noise)?;

        // The seed is a Gaussian width; convert it to the equivalent alpha at the fixed β.
        let fwhm_est = fit.sigma_est * FWHM_TO_SIGMA;
        let initial_alpha =
            fwhm_beta_to_alpha(fwhm_est, config.fixed_beta).clamp(0.5, grid.radius as f32);

        let initial_params: [f64; 5] = [
            fit.local_pos.x,
            fit.local_pos.y,
            fit.amplitude_seed(background),
            initial_alpha as f64,
            background as f64,
        ];

        let model = MoffatFixedBeta::new(grid.radius as f64, config.fixed_beta as f64);
        let result = model.fit(fit.data(grid), initial_params, &config.lm);

        let [x0, y0, _, alpha, _] = result.params;
        let result_pos = fit.to_image(x0, y0);

        if !fit_is_plausible(result_pos, pos, grid.radius, [alpha]) {
            return None;
        }

        Some(Self {
            pos: result_pos,
            fwhm: alpha_beta_to_fwhm(alpha as f32, config.fixed_beta),
            converged: result.converged,
            #[cfg(test)]
            debug: MoffatFitDebug::of(&result),
        })
    }
}

/// Convert Moffat alpha and beta to FWHM.
/// FWHM = 2 * alpha * sqrt(2^(1/beta) - 1)
#[inline]
pub(super) fn alpha_beta_to_fwhm(alpha: f32, beta: f32) -> f32 {
    2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt()
}

/// Convert FWHM and beta to Moffat alpha.
/// alpha = FWHM / (2 * sqrt(2^(1/beta) - 1))
#[inline]
pub(super) fn fwhm_beta_to_alpha(fwhm: f32, beta: f32) -> f32 {
    fwhm / (2.0 * (2.0f32.powf(1.0 / beta) - 1.0).sqrt())
}

#[cfg(test)]
mod internals {
    use crate::stacking::star_detection::centroid::lm_optimizer::LMResult;
    use crate::stacking::star_detection::centroid::moffat_fit::{
        MoffatFit, MoffatFitDebug, MoffatFixedBeta, fast_pow_neg,
    };

    impl MoffatFitDebug {
        /// Derive the diagnostics from the optimizer's report. Gated with the struct, so a
        /// release build runs none of this.
        pub(super) fn of(result: &LMResult<5>) -> Self {
            let [_, _, amplitude, alpha, background] = result.params;
            Self {
                amplitude: amplitude as f32,
                alpha: alpha as f32,
                background: background as f32,
                iterations: result.iterations,
            }
        }
    }

    impl MoffatFit {
        /// Exposes `MoffatFitDebug::alpha` to `centroid::tests`, which sits outside
        /// `moffat_fit` and so can name neither the private `debug` field nor its type.
        pub(crate) fn debug_alpha(&self) -> f32 {
            self.debug.alpha
        }
    }

    impl MoffatFixedBeta {
        /// The Jacobian row alone, derived independently of
        /// [`MoffatFixedBeta::evaluate_and_jacobian`]'s fused form.
        ///
        /// Production takes only the fused path; this exists so
        /// `test_moffat_fixed_beta_evaluate_and_jacobian_consistency` has a second derivation of
        /// the same algebra to check it against. Keep the two written out separately — sharing a
        /// helper between them would make the test compare an expression with itself.
        pub(super) fn jacobian_row(&self, x: f64, y: f64, params: &[f64; 5]) -> [f64; 5] {
            let [x0, y0, amp, alpha, _bg] = *params;
            let alpha2 = alpha * alpha;
            let dx = x - x0;
            let dy = y - y0;
            let r2 = dx * dx + dy * dy;
            let u = 1.0 + r2 / alpha2;
            let u_neg_beta = fast_pow_neg(u, self.pow_strategy);
            let u_neg_beta_m1 = u_neg_beta / u;
            let common = 2.0 * amp * self.beta / alpha2 * u_neg_beta_m1;

            [
                common * dx,         // df/dx0
                common * dy,         // df/dy0
                u_neg_beta,          // df/damp
                common * r2 / alpha, // df/dalpha
                1.0,                 // df/dbg
            ]
        }
    }
}
