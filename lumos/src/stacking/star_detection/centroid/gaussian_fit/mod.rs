//! 2D Gaussian fitting for high-precision centroid computation.
//!
//! Levenberg-Marquardt fit of an elliptical, rotated Gaussian held by its inverse covariance:
//! `f(x, y) = A·exp(−½(a·dx² + 2b·dx·dy + c·dy²)) + B`, with `a·c − b² > 0`.
//!
//! The inverse covariance rather than `(σx, σy, θ)`: it has no angle to wrap, and no angle at all
//! to be undefined when the star is round — `b = 0, a = c` is an ordinary point of the parameter
//! space, where `θ` would be degenerate and its derivative zero. The principal widths and the
//! eccentricity follow from `(a, b, c)` after the fit.
//!
//! Uses f64 throughout the fitting pipeline for numerical stability,
//! achieving ~0.01 pixel centroid accuracy.

mod simd;

use crate::stacking::star_detection::centroid::covariance::Cov2;
use crate::stacking::star_detection::centroid::fit_is_plausible;
use crate::stacking::star_detection::centroid::lm_optimizer::{
    FitData, LMConfig, LMModel, ModelSample, NormalEquations,
};
use crate::stacking::star_detection::centroid::stamp::FitNoise;
use crate::stacking::star_detection::centroid::stamp::StampFit;
use crate::stacking::star_detection::centroid::stamp::StampGrid;
use glam::DVec2;
use imaginarium::Buffer2;

/// Configuration for Gaussian fitting.
pub(super) type GaussianFitConfig = LMConfig;

/// How close to singular the fitted inverse covariance may come: `b² ≤ (1 − MARGIN)·a·c`. The
/// principal variances' ratio is then at most `(1 + √(1 − m)) / (1 − √(1 − m))` ≈ 4·10⁴ — far past
/// any star — while `a·c − b²` stays a positive share of `a·c` that the covariance can be
/// inverted from.
const DEFINITENESS_MARGIN: f64 = 1e-4;

/// A converged-or-not 2D Gaussian fitted to one star stamp: where its centre landed and its
/// shape. Build one with [`GaussianFit::new`].
#[derive(Debug, Clone, Copy)]
pub(super) struct GaussianFit {
    /// Position of Gaussian center (sub-pixel).
    pub(super) pos: DVec2,
    /// The profile's covariance, in px².
    pub(super) covariance: Cov2,
    /// Whether the fit converged.
    pub(super) converged: bool,
    /// Fit diagnostics that no production caller reads — `measure_star` only uses
    /// `pos`/`covariance`/`converged` — but that tests need to verify LM convergence against
    /// synthetic ground truth. Gated rather than carried and ignored, so a release build
    /// neither stores them nor runs the arithmetic that fills them.
    #[cfg(test)]
    debug: internals::GaussianFitDebug,
}

/// The elliptical Gaussian for L-M optimization (7 parameters):
/// `[x0, y0, amplitude, a, b, c, background]`.
#[derive(Debug)]
struct Gaussian2D {
    /// The widest σ the fit may take, in px: the stamp radius.
    max_sigma: f64,
    /// The smallest amplitude the fit may take; see [`StampFit::min_amplitude`].
    min_amplitude: f64,
}

/// The narrowest σ the fit may take, in px.
const MIN_SIGMA: f64 = 0.5;

impl Gaussian2D {
    /// The range `a` and `c` are held to: `1/σ²` for σ between [`MIN_SIGMA`] and the stamp radius.
    fn curvature_range(&self) -> (f64, f64) {
        (
            1.0 / (self.max_sigma * self.max_sigma),
            1.0 / (MIN_SIGMA * MIN_SIGMA),
        )
    }
}

impl LMModel<7> for Gaussian2D {
    #[inline]
    fn evaluate(&self, x: f64, y: f64, params: &[f64; 7]) -> f64 {
        let [x0, y0, amp, a, b, c, bg] = *params;
        let dx = x - x0;
        let dy = y - y0;
        let q = dx * (a * dx + b * dy) + dy * (b * dx + c * dy);
        amp * (-0.5 * q).exp() + bg
    }

    /// `∂f/∂x0 = A·E·(a·dx + b·dy)`, `∂f/∂y0 = A·E·(b·dx + c·dy)`, `∂f/∂A = E`,
    /// `∂f/∂a = −½A·E·dx²`, `∂f/∂b = −A·E·dx·dy`, `∂f/∂c = −½A·E·dy²`, `∂f/∂B = 1`,
    /// with `E = exp(−½(a·dx² + 2b·dx·dy + c·dy²))`.
    #[inline]
    fn evaluate_and_jacobian(&self, x: f64, y: f64, params: &[f64; 7]) -> ModelSample<7> {
        let [x0, y0, amp, a, b, c, bg] = *params;
        let dx = x - x0;
        let dy = y - y0;
        let t = a * dx + b * dy;
        let u = b * dx + c * dy;
        let exp_val = (-0.5 * (dx * t + dy * u)).exp();
        let amp_exp = amp * exp_val;
        let half_amp_exp = -0.5 * amp_exp;
        ModelSample {
            value: amp_exp + bg,
            jacobian: [
                amp_exp * t,
                amp_exp * u,
                exp_val,
                half_amp_exp * dx * dx,
                half_amp_exp * dx * (dy + dy),
                half_amp_exp * dy * dy,
                1.0,
            ],
        }
    }

    /// Amplitude at least `min_amplitude`, `a` and `c` within [`Gaussian2D::curvature_range`],
    /// and `b` held to [`DEFINITENESS_MARGIN`] of singular, so every step leaves a profile that is
    /// a Gaussian.
    #[inline]
    fn constrain(&self, params: &mut [f64; 7]) {
        let (min_curvature, max_curvature) = self.curvature_range();
        params[2] = params[2].max(self.min_amplitude);
        params[3] = params[3].clamp(min_curvature, max_curvature);
        params[5] = params[5].clamp(min_curvature, max_curvature);
        let b_max = ((1.0 - DEFINITENESS_MARGIN) * params[3] * params[5]).sqrt();
        params[4] = params[4].clamp(-b_max, b_max);
    }

    fn batch_build_normal_equations(
        &self,
        data: FitData<'_>,
        params: &[f64; 7],
    ) -> NormalEquations<7> {
        simd::batch_build_normal_equations(self, data, params)
            .unwrap_or_else(|| NormalEquations::from_scalar_pass(self, data, params))
    }

    fn batch_compute_chi2(&self, data: FitData<'_>, params: &[f64; 7]) -> f64 {
        simd::batch_compute_chi2(self, data, params)
            .unwrap_or_else(|| self.accumulate_chi2(data, params, 0..data.len()))
    }
}

impl GaussianFit {
    /// Fit the elliptical Gaussian to a star stamp via Levenberg-Marquardt (f64 throughout,
    /// ~0.01 px centroid accuracy). When `noise` is set, each pixel is weighted by `1/σ²` from the
    /// CCD noise model so the shot-noisy bright core doesn't bias the fit; `None` is a plain
    /// unweighted fit.
    ///
    /// `None` also when the stamp falls outside the frame, holds too few pixels to constrain seven
    /// parameters, or the fit lands somewhere the data did not support: a centre that wandered off
    /// ([`fit_is_plausible`]), or an amplitude or shape pinned at one of
    /// [`Gaussian2D::constrain`]'s bounds.
    pub(super) fn new(
        pixels: &Buffer2<f32>,
        pos: DVec2,
        grid: &StampGrid,
        background: f32,
        noise: Option<FitNoise>,
        config: &GaussianFitConfig,
    ) -> Option<Self> {
        let mut fit = StampFit::prepare::<7>(pixels, pos, grid, background, noise)?;

        // A round seed; the fit finds the elongation and its angle.
        let curvature = 1.0 / (f64::from(fit.sigma_est) * f64::from(fit.sigma_est));
        let initial_params: [f64; 7] = [
            fit.local_pos.x,
            fit.local_pos.y,
            fit.amplitude_seed(background),
            curvature,
            0.0,
            curvature,
            f64::from(background),
        ];

        let model = Gaussian2D {
            max_sigma: grid.radius as f64,
            min_amplitude: fit.min_amplitude(background),
        };
        let result = fit.fit(&model, grid, initial_params, config);

        let [x0, y0, amplitude, a, b, c, _] = result.params;
        let result_pos = fit.to_image(x0, y0);
        let (min_curvature, max_curvature) = model.curvature_range();
        let b_max = ((1.0 - DEFINITENESS_MARGIN) * a * c).sqrt();
        let shape_free = amplitude > model.min_amplitude
            && [a, c]
                .iter()
                .all(|&k| k > min_curvature && k < max_curvature)
            && b.abs() < b_max;
        if !shape_free || !fit_is_plausible(result_pos, pos, grid.radius) {
            return None;
        }
        let det = a * c - b * b;
        Some(Self {
            pos: result_pos,
            covariance: Cov2 {
                xx: c / det,
                yy: a / det,
                xy: -b / det,
            },
            converged: result.converged,
            #[cfg(test)]
            debug: internals::GaussianFitDebug::of(&result, fit.stamp.z.len()),
        })
    }
}

#[cfg(test)]
mod internals {
    /// Fit diagnostics kept for tests; see [`GaussianFit::debug`].
    #[derive(Debug, Clone, Copy)]
    pub(super) struct GaussianFitDebug {
        /// Amplitude of Gaussian.
        pub(super) amplitude: f32,
        /// Background level.
        pub(super) background: f32,
        /// RMS residual of fit.
        pub(super) rms_residual: f32,
        /// Number of iterations used.
        pub(super) iterations: usize,
    }

    use glam::Vec2;

    use crate::stacking::star_detection::centroid::gaussian_fit::{Gaussian2D, GaussianFit};
    use crate::stacking::star_detection::centroid::lm_optimizer::LMResult;

    impl GaussianFit {
        /// The profile's σ along x and along y — its covariance's diagonal, which for an
        /// axis-aligned fixture are the principal widths.
        pub(super) fn axis_sigma(&self) -> Vec2 {
            Vec2::new(
                self.covariance.xx.sqrt() as f32,
                self.covariance.yy.sqrt() as f32,
            )
        }
    }

    impl GaussianFitDebug {
        /// Derive the diagnostics from the optimizer's report, where `n` is the sample count the
        /// χ² was summed over. Gated with the struct, so a release build runs none of this.
        pub(super) fn of(result: &LMResult<7>, n: usize) -> Self {
            let [_, _, amplitude, _, _, _, background] = result.params;
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
        /// Production takes only the fused path; this exists so the consistency test has a second
        /// derivation of the same algebra to check it against. Keep the two written out
        /// separately — sharing a helper between them would make the test compare an expression
        /// with itself.
        pub(super) fn jacobian_row(x: f64, y: f64, params: &[f64; 7]) -> [f64; 7] {
            let [x0, y0, amp, a, b, c, _bg] = *params;
            let dx = x - x0;
            let dy = y - y0;
            let quadratic = a * dx * dx + 2.0 * b * dx * dy + c * dy * dy;
            let e = (-quadratic / 2.0).exp();
            [
                amp * e * (a * dx + b * dy),
                amp * e * (b * dx + c * dy),
                e,
                -amp * e * dx * dx / 2.0,
                -amp * e * dx * dy,
                -amp * e * dy * dy / 2.0,
                1.0,
            ]
        }
    }
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
