//! 2D Moffat profile fitting for high-precision centroid computation.
//!
//! The Moffat profile is a better model for stellar PSFs than Gaussian because
//! it has extended wings that match atmospheric seeing:
//!
//! f(x,y) = A × (1 + ((x-x₀)²+(y-y₀)²)/α²)^(-β) + B
//!
//! where α is the core width and β controls the wing slope (typically 2.5-4.5), integrated over
//! each pixel.
//!
//! Uses f64 throughout the fitting pipeline for numerical stability. Each fit reports its
//! position's standard error, from `(JᵀWJ)⁻¹·χ²/(n − p)`, rather than a fixed accuracy.

mod simd;

use crate::math::fwhm::{FWHM_PER_SIGMA, alpha_beta_to_fwhm, fwhm_beta_to_alpha, sigma_to_fwhm};
use crate::math::lm_controller::NormalEquations;
use crate::math::pixel_quadrature::{AnalyticProfile, PixelQuadrature};
use crate::simd::Kernel;
use crate::star_detection::centroid::lm_optimizer::{FitData, LMModel};
use crate::star_detection::centroid::moffat_fit::simd::MoffatBatch;
use crate::star_detection::centroid::simd::{Chi2Kernel, NormalEquationsKernel};
use crate::star_detection::centroid::stamp::StampFit;
use crate::star_detection::centroid::stamp::StampGrid;
use crate::star_detection::centroid::star_noise::StarNoise;
use crate::star_detection::centroid::{
    MIN_PROFILE_SIGMA, PIXEL_MEAN_TOLERANCE, fit_is_plausible, position_sigma,
};
use glam::DVec2;
use imaginarium::Buffer2;

/// A converged 2D Moffat profile fitted to one star stamp: where its centre landed and
/// the FWHM that follows from its alpha and beta. Build one with [`MoffatFit::new`].
#[derive(Debug, Clone, Copy)]
pub(super) struct MoffatFit {
    /// Position of profile center (sub-pixel).
    pub(super) pos: DVec2,
    /// FWHM computed from alpha and beta.
    pub(super) fwhm: f32,
    /// `√((σ_x² + σ_y²)/2)` of `pos`, from the fit's `(JᵀWJ)⁻¹·χ²/(n − p)`.
    pub(super) position_sigma: f64,
    /// Fit diagnostics that no production caller reads — `measure_star` only uses
    /// `pos`/`fwhm` — but that tests need to verify LM convergence against
    /// synthetic ground truth. Gated rather than carried and ignored, so a release build
    /// neither stores them nor runs the arithmetic that fills them.
    #[cfg(test)]
    debug: internals::MoffatFitDebug,
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

/// `u^n` by squaring — the same multiplications, in the same order, as the vector kernel's powers,
/// so a lane and the scalar path agree bit for bit at every `n`.
#[inline]
fn int_pow(u: f64, n: u32) -> f64 {
    let (mut result, mut base, mut exp) = (1.0, u, n);
    while exp > 0 {
        if exp & 1 == 1 {
            result *= base;
        }
        base *= base;
        exp >>= 1;
    }
    result
}

/// Select optimal strategy for computing u^(-beta).
#[expect(
    clippy::cast_sign_loss,
    reason = "validate holds β in (1, 10], so its doubled, rounded order is positive"
)]
fn select_pow_strategy(beta: f64) -> PowStrategy {
    let rounded = (beta * 2.0).round();
    let is_half_int = (beta * 2.0 - rounded).abs() < 1e-10;

    if is_half_int {
        let doubled = rounded as i64;
        if doubled % 2 == 0 {
            PowStrategy::Int {
                n: (doubled / 2) as u32,
            }
        } else {
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
    /// The smallest amplitude the fit may take; see [`StampFit::min_amplitude`].
    min_amplitude: f64,
    /// The narrowest α the fit may take, in px: the profile of the FWHM a Gaussian of
    /// [`MIN_PROFILE_SIGMA`] has. The widest is the stamp radius.
    min_alpha: f64,
    /// The midpoint until [`StampFit::fit`] sets the order the profile needs.
    quadrature: PixelQuadrature,
}

impl MoffatFixedBeta {
    fn new(stamp_radius: f64, beta: f64, min_amplitude: f64) -> Self {
        let min_fwhm = FWHM_PER_SIGMA * MIN_PROFILE_SIGMA;
        Self {
            stamp_radius,
            beta,
            pow_strategy: select_pow_strategy(beta),
            min_amplitude,
            min_alpha: min_fwhm / (2.0 * (2f64.powf(1.0 / beta) - 1.0).sqrt()),
            quadrature: PixelQuadrature::gauss_legendre(1),
        }
    }
}

impl LMModel<5> for MoffatFixedBeta {
    #[inline]
    fn point(&self, x: f64, y: f64, params: &[f64; 5]) -> f64 {
        let [x0, y0, amp, alpha, bg] = *params;
        let r2 = (x - x0).powi(2) + (y - y0).powi(2);
        let u = 1.0 + r2 / (alpha * alpha);
        amp * fast_pow_neg(u, self.pow_strategy) + bg
    }

    fn quadrature(&self) -> &PixelQuadrature {
        &self.quadrature
    }

    fn integrate_at(&mut self, order: usize) {
        self.quadrature = PixelQuadrature::gauss_legendre(order);
    }

    /// From α and β; at the narrowest α the fit admits, with β just above 1, the order is 11.
    fn sufficient_order(&self, params: &[f64; 5]) -> usize {
        let profile = AnalyticProfile::Moffat {
            alpha: params[3],
            beta: self.beta,
        };
        PixelQuadrature::sufficient_order(profile, PIXEL_MEAN_TOLERANCE)
            .expect("a Moffat of FWHM 1.18 or wider at β above 1 needs order 11")
    }

    #[inline]
    fn constrain(&self, params: &mut [f64; 5]) {
        params[2] = params[2].max(self.min_amplitude);
        params[3] = params[3].clamp(self.min_alpha, self.stamp_radius);
    }

    fn batch_build_normal_equations(
        &self,
        data: FitData<'_>,
        params: &[f64; 5],
    ) -> NormalEquations<5> {
        NormalEquationsKernel {
            model: MoffatBatch::new(self, *params),
            data,
            quadrature: &self.quadrature,
        }
        .dispatch()
    }

    fn batch_compute_chi2(&self, data: FitData<'_>, params: &[f64; 5]) -> f64 {
        Chi2Kernel {
            model: MoffatBatch::new(self, *params),
            data,
            quadrature: &self.quadrature,
        }
        .dispatch()
    }
}

impl MoffatFit {
    /// Fit a 2D Moffat profile to a star stamp via Levenberg-Marquardt (f64 throughout). When
    /// `noise` is set, each pixel is weighted by `1/σ²` from the CCD noise model so the
    /// shot-noisy bright core doesn't bias the fit; `None` is a plain unweighted fit.
    ///
    /// `None` also when the stamp falls outside the frame, holds too few pixels to constrain five
    /// parameters, or the fit lands somewhere the data did not support: a centre that wandered off
    /// ([`fit_is_plausible`]), or an amplitude or α pinned at one of
    /// [`MoffatFixedBeta::constrain`]'s bounds.
    pub(super) fn new(
        pixels: &Buffer2<f32>,
        pos: DVec2,
        grid: &StampGrid,
        background: f32,
        noise: Option<StarNoise>,
        beta: f32,
    ) -> Option<Self> {
        // Fixed-β Moffat fits 5 parameters [x0, y0, amplitude, alpha, background].
        let mut fit = StampFit::prepare::<5>(pixels, pos, grid, background, noise)?;
        let amplitude_seed = fit.amplitude_seed()?;

        let mut model = MoffatFixedBeta::new(
            grid.radius as f64,
            f64::from(beta),
            StampFit::min_amplitude(amplitude_seed),
        );

        // The seed is a Gaussian width; convert it to the equivalent alpha at the fixed β.
        let fwhm_est = sigma_to_fwhm(fit.sigma_est);
        let initial_alpha = f64::from(fwhm_beta_to_alpha(fwhm_est, beta))
            .clamp(model.min_alpha, model.stamp_radius);

        let initial_params: [f64; 5] = [
            fit.local_pos.x,
            fit.local_pos.y,
            amplitude_seed,
            initial_alpha,
            f64::from(background),
        ];

        let result = fit.fit(&mut model, grid, initial_params)?;

        let [x0, y0, amplitude, alpha, _] = result.params;
        let result_pos = fit.to_image(x0, y0);

        let shape_free = amplitude > model.min_amplitude
            && alpha > model.min_alpha
            && alpha < model.stamp_radius;
        if !shape_free || !fit_is_plausible(result_pos, pos, grid.radius) {
            return None;
        }

        Some(Self {
            pos: result_pos,
            position_sigma: position_sigma(&result, fit.stamp.z.len())?,
            fwhm: alpha_beta_to_fwhm(alpha as f32, beta),
            #[cfg(test)]
            debug: internals::MoffatFitDebug::of(&result, model.quadrature.nodes().len()),
        })
    }
}

#[cfg(test)]
mod internals {
    use crate::math::lm_controller::LmFit;
    use crate::star_detection::centroid::lm_optimizer::internals::{ModelJacobian, ModelSample};
    use crate::star_detection::centroid::moffat_fit::{MoffatFixedBeta, fast_pow_neg};

    impl ModelJacobian<5> for MoffatFixedBeta {
        fn point_and_jacobian(&self, x: f64, y: f64, params: &[f64; 5]) -> ModelSample<5> {
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

            ModelSample {
                value: model_val,
                jacobian: [
                    common * dx,         // df/dx0
                    common * dy,         // df/dy0
                    u_neg_beta,          // df/damp
                    common * r2 / alpha, // df/dalpha
                    1.0,                 // df/dbg
                ],
            }
        }
    }

    /// Fit diagnostics kept for tests; see [`MoffatFit::debug`].
    #[derive(Debug, Clone, Copy)]
    pub(super) struct MoffatFitDebug {
        /// Amplitude of profile.
        pub(super) amplitude: f32,
        /// Core width parameter (alpha).
        pub(super) alpha: f32,
        /// Background level.
        pub(super) background: f32,
        /// The quadrature order the fit ended at.
        pub(super) order: usize,
    }

    impl MoffatFitDebug {
        /// Derive the diagnostics from the optimizer's report. Gated with the struct, so a
        /// release build runs none of this.
        pub(super) fn of(result: &LmFit<5>, order: usize) -> Self {
            let [_, _, amplitude, alpha, background] = result.params;
            Self {
                amplitude: amplitude as f32,
                alpha: alpha as f32,
                background: background as f32,
                order,
            }
        }
    }

    impl MoffatFixedBeta {
        /// The Jacobian row alone at one point, derived independently of
        /// [`MoffatFixedBeta::point_and_jacobian`]'s fused form.
        ///
        /// The vector kernel mirrors the fused path; this exists so
        /// `moffat_fixed_beta_evaluate_and_jacobian_consistency` has a second derivation of
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

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
