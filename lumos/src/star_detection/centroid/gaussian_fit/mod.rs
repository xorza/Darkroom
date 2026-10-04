//! 2D Gaussian fitting for high-precision centroid computation.
//!
//! Levenberg-Marquardt fit of an elliptical, rotated Gaussian held by its inverse covariance:
//! `f(x, y) = A·exp(−½(a·dx² + 2b·dx·dy + c·dy²)) + B`, with `a·c − b² > 0`, integrated over each
//! pixel.
//!
//! The inverse covariance rather than `(σx, σy, θ)`: it has no angle to wrap, and no angle at all
//! to be undefined when the star is round — `b = 0, a = c` is an ordinary point of the parameter
//! space, where `θ` would be degenerate and its derivative zero. The principal widths and the
//! eccentricity follow from `(a, b, c)` after the fit.
//!
//! Uses f64 throughout the fitting pipeline for numerical stability. Each fit reports its
//! position's standard error, from `(JᵀWJ)⁻¹·χ²/(n − p)`, rather than a fixed accuracy.

mod simd;

use crate::math::lm_controller::NormalEquations;
use crate::math::pixel_quadrature::{AnalyticProfile, PixelQuadrature};
use crate::simd::Kernel;
use crate::star_detection::centroid::covariance::Cov2;
use crate::star_detection::centroid::gaussian_fit::simd::GaussianBatch;
use crate::star_detection::centroid::lm_optimizer::{FitData, LMModel};
use crate::star_detection::centroid::simd::{Chi2Kernel, NormalEquationsKernel};
use crate::star_detection::centroid::stamp::StampFit;
use crate::star_detection::centroid::stamp::StampGrid;
use crate::star_detection::centroid::star_noise::StarNoise;
use crate::star_detection::centroid::{
    MIN_PROFILE_SIGMA, PIXEL_MEAN_TOLERANCE, fit_is_plausible, position_sigma,
};
use glam::DVec2;
use imaginarium::Buffer2;
use std::ops::RangeInclusive;

/// A converged 2D Gaussian fitted to one star stamp: where its centre landed and its
/// shape. Build one with [`GaussianFit::new`].
#[derive(Debug, Clone, Copy)]
pub(super) struct GaussianFit {
    /// Position of Gaussian center (sub-pixel).
    pub(super) pos: DVec2,
    /// The profile's covariance, in px².
    pub(super) covariance: Cov2,
    /// `√((σ_x² + σ_y²)/2)` of `pos`, from the fit's `(JᵀWJ)⁻¹·χ²/(n − p)`.
    pub(super) position_sigma: f64,
    /// Fit diagnostics that no production caller reads — `measure_star` only uses
    /// `pos`/`covariance` — but that tests need to verify LM convergence against
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
    /// The midpoint until [`StampFit::fit`] sets the order the profile needs.
    quadrature: PixelQuadrature,
}

impl Gaussian2D {
    fn new(max_sigma: f64, min_amplitude: f64) -> Self {
        Self {
            max_sigma,
            min_amplitude,
            quadrature: PixelQuadrature::gauss_legendre(1),
        }
    }

    /// The range the inverse covariance's eigenvalues are held to: `1/σ²` for a principal σ
    /// between [`MIN_PROFILE_SIGMA`] and the stamp radius. Its diagonal lies between the eigenvalues, so
    /// `a` and `c` are held to it too.
    fn curvature_range(&self) -> RangeInclusive<f64> {
        1.0 / (self.max_sigma * self.max_sigma)..=1.0 / (MIN_PROFILE_SIGMA * MIN_PROFILE_SIGMA)
    }

    /// The largest `|b|` beside `a` and `c` within [`Self::curvature_range`] that keeps both
    /// eigenvalues `½(a + c) ± √(¼(a − c)² + b²)` in it: `b² ≤ (K − a)(K − c)` bounds the larger
    /// by `K`, and `b² ≤ (a − k)(c − k)` the smaller by `k > 0`, which keeps the matrix definite.
    /// [`LMModel::constrain`] holds the fit there, so a fit pinned at it is told apart from a free
    /// one.
    fn max_cross_term(&self, a: f64, c: f64) -> f64 {
        let curvature = self.curvature_range();
        let (k, big_k) = (*curvature.start(), *curvature.end());
        ((big_k - a) * (big_k - c)).min((a - k) * (c - k)).sqrt()
    }
}

impl LMModel<7> for Gaussian2D {
    #[inline]
    fn point(&self, x: f64, y: f64, params: &[f64; 7]) -> f64 {
        let [x0, y0, amp, a, b, c, bg] = *params;
        let dx = x - x0;
        let dy = y - y0;
        let q = dx * (a * dx + b * dy) + dy * (b * dx + c * dy);
        amp * (-0.5 * q).exp() + bg
    }

    fn quadrature(&self) -> &PixelQuadrature {
        &self.quadrature
    }

    fn integrate_at(&mut self, order: usize) {
        self.quadrature = PixelQuadrature::gauss_legendre(order);
    }

    /// From the narrower of the σ along each pixel axis at a fixed offset along the other,
    /// `1/√a` and `1/√c`; within the curvature range it is at least [`MIN_PROFILE_SIGMA`], where
    /// the order is 7.
    fn sufficient_order(&self, params: &[f64; 7]) -> usize {
        let sigma = 1.0 / params[3].max(params[5]).sqrt();
        PixelQuadrature::sufficient_order(AnalyticProfile::Gaussian { sigma }, PIXEL_MEAN_TOLERANCE)
            .expect("a Gaussian of σ 0.5 or wider needs order 7")
    }

    /// Amplitude at least `min_amplitude`, and the principal widths within
    /// [`Gaussian2D::curvature_range`]: `a` and `c` within it, then `b` within
    /// [`Gaussian2D::max_cross_term`], so every step leaves a profile that is a Gaussian.
    #[inline]
    fn constrain(&self, params: &mut [f64; 7]) {
        let curvature = self.curvature_range();
        params[2] = params[2].max(self.min_amplitude);
        params[3] = params[3].clamp(*curvature.start(), *curvature.end());
        params[5] = params[5].clamp(*curvature.start(), *curvature.end());
        let b_max = self.max_cross_term(params[3], params[5]);
        params[4] = params[4].clamp(-b_max, b_max);
    }

    fn batch_build_normal_equations(
        &self,
        data: FitData<'_>,
        params: &[f64; 7],
    ) -> NormalEquations<7> {
        NormalEquationsKernel {
            model: GaussianBatch::new(*params),
            data,
            quadrature: &self.quadrature,
        }
        .dispatch()
    }

    fn batch_compute_chi2(&self, data: FitData<'_>, params: &[f64; 7]) -> f64 {
        Chi2Kernel {
            model: GaussianBatch::new(*params),
            data,
            quadrature: &self.quadrature,
        }
        .dispatch()
    }
}

impl GaussianFit {
    /// Fit the elliptical Gaussian to a star stamp via Levenberg-Marquardt, in f64. When `noise` is
    /// set, each pixel is weighted by `1/σ²` from the
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
        noise: Option<StarNoise>,
    ) -> Option<Self> {
        let mut fit = StampFit::prepare::<7>(pixels, pos, grid, background, noise)?;
        let amplitude_seed = fit.amplitude_seed()?;

        // A round seed; the fit finds the elongation and its angle.
        let curvature = 1.0 / (f64::from(fit.sigma_est) * f64::from(fit.sigma_est));
        let initial_params: [f64; 7] = [
            fit.local_pos.x,
            fit.local_pos.y,
            amplitude_seed,
            curvature,
            0.0,
            curvature,
            f64::from(background),
        ];

        let mut model =
            Gaussian2D::new(grid.radius as f64, StampFit::min_amplitude(amplitude_seed));
        let result = fit.fit(&mut model, grid, initial_params)?;

        let [x0, y0, amplitude, a, b, c, _] = result.params;
        let result_pos = fit.to_image(x0, y0);
        let curvature = model.curvature_range();
        let b_max = model.max_cross_term(a, c);
        let shape_free = amplitude > model.min_amplitude
            && [a, c]
                .iter()
                .all(|&k| k > *curvature.start() && k < *curvature.end())
            && b.abs() < b_max;
        if !shape_free || !fit_is_plausible(result_pos, pos, grid.radius) {
            return None;
        }
        let det = a * c - b * b;
        Some(Self {
            pos: result_pos,
            position_sigma: position_sigma(&result, fit.stamp.z.len())?,
            covariance: Cov2 {
                xx: c / det,
                yy: a / det,
                xy: -b / det,
            },
            #[cfg(test)]
            debug: internals::GaussianFitDebug::of(
                &result,
                fit.stamp.z.len(),
                model.quadrature.nodes().len(),
            ),
        })
    }
}

#[cfg(test)]
mod internals {
    use glam::Vec2;

    use crate::math::lm_controller::LmFit;
    use crate::star_detection::centroid::gaussian_fit::{Gaussian2D, GaussianFit};
    use crate::star_detection::centroid::lm_optimizer::internals::{ModelJacobian, ModelSample};

    impl ModelJacobian<7> for Gaussian2D {
        /// `∂f/∂x0 = A·E·(a·dx + b·dy)`, `∂f/∂y0 = A·E·(b·dx + c·dy)`, `∂f/∂A = E`,
        /// `∂f/∂a = −½A·E·dx²`, `∂f/∂b = −A·E·dx·dy`, `∂f/∂c = −½A·E·dy²`, `∂f/∂B = 1`,
        /// with `E = exp(−½(a·dx² + 2b·dx·dy + c·dy²))`.
        fn point_and_jacobian(&self, x: f64, y: f64, params: &[f64; 7]) -> ModelSample<7> {
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
    }

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
        /// The quadrature order the fit ended at.
        pub(super) order: usize,
    }

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
        pub(super) fn of(result: &LmFit<7>, n: usize, order: usize) -> Self {
            let [_, _, amplitude, _, _, _, background] = result.params;
            Self {
                amplitude: amplitude as f32,
                background: background as f32,
                rms_residual: (result.chi2 / n as f64).sqrt() as f32,
                iterations: result.iterations,
                order,
            }
        }
    }

    impl Gaussian2D {
        /// The Jacobian row alone at one point, derived independently of
        /// [`Gaussian2D::point_and_jacobian`]'s fused form.
        ///
        /// The vector kernel mirrors the fused path; this exists so the consistency test has a second
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
