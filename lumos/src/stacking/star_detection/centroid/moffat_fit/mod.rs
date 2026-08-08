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

#[cfg(target_arch = "x86_64")]
mod simd_avx2;

#[cfg(target_arch = "aarch64")]
mod simd_neon;

use crate::math::FWHM_TO_SIGMA;
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

/// Result of 2D Moffat profile fitting.
#[derive(Debug, Clone, Copy)]
pub(super) struct MoffatFitResult {
    /// Position of profile center (sub-pixel).
    pub(super) pos: Vec2,
    /// FWHM computed from alpha and beta.
    pub(super) fwhm: f32,
    /// Whether the fit converged.
    pub(super) converged: bool,
    /// Fit diagnostics (amplitude/alpha/background/iteration count) that no
    /// production caller reads — `measure_star` only uses `pos`/`fwhm`/`converged` —
    /// but that tests need to verify LM convergence against synthetic ground truth.
    #[allow(dead_code)] // read only by tests
    debug: MoffatFitDebug,
}

/// Fit diagnostics kept for tests; see [`MoffatFitResult::debug`].
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // read only by tests
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
        // The SIMD kernels are unweighted-only, so a weighted fit takes the scalar path.
        if data.weights.is_none() {
            #[cfg(target_arch = "x86_64")]
            if imaginarium::cpu_features::has_avx2_fma() {
                // SAFETY: AVX2+FMA availability checked above.
                return unsafe {
                    simd_avx2::batch_build_normal_equations_avx2(
                        self, data.x, data.y, data.z, params,
                    )
                };
            }
            #[cfg(target_arch = "aarch64")]
            {
                // SAFETY: NEON is unconditionally available on aarch64.
                return unsafe {
                    simd_neon::batch_build_normal_equations_neon(
                        self, data.x, data.y, data.z, params,
                    )
                };
            }
        }
        build_normal_equations_scalar(self, data, params)
    }

    fn batch_compute_chi2(&self, data: FitData, params: &[f64; 5]) -> f64 {
        // The SIMD kernels are unweighted-only, so a weighted fit takes the scalar path.
        if data.weights.is_none() {
            #[cfg(target_arch = "x86_64")]
            if imaginarium::cpu_features::has_avx2_fma() {
                // SAFETY: AVX2+FMA availability checked above.
                return unsafe {
                    simd_avx2::batch_compute_chi2_avx2(self, data.x, data.y, data.z, params)
                };
            }
            #[cfg(target_arch = "aarch64")]
            {
                // SAFETY: NEON is unconditionally available on aarch64.
                return unsafe {
                    simd_neon::batch_compute_chi2_neon(self, data.x, data.y, data.z, params)
                };
            }
        }
        accumulate_chi2(self, data, params, 0..data.len())
    }
}

/// Fit a 2D Moffat profile to a star stamp via Levenberg-Marquardt (f64 throughout). When
/// `noise` is set, each pixel is weighted by `1/σ²` from the CCD noise model so the
/// shot-noisy bright core doesn't bias the fit (PR1); `None` is a plain unweighted fit.
pub(super) fn fit_moffat_2d(
    pixels: &Buffer2<f32>,
    pos: Vec2,
    stamp_radius: usize,
    background: f32,
    noise: Option<FitNoise>,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let stamp = extract_stamp(pixels, pos, stamp_radius)?;

    // Fixed-β Moffat fits 5 parameters [x0, y0, amplitude, alpha, background].
    let n = stamp.x.len();
    if n < 6 {
        return None;
    }

    // Convert stamp data to f64 for fitting. Stack-allocated (stamp size is bounded
    // by MAX_STAMP_PIXELS), so the parallel per-star fit loop makes no heap allocations.
    let data_x: ArrayVec<f64, MAX_STAMP_PIXELS> = stamp.x.iter().map(|&v| v as f64).collect();
    let data_y: ArrayVec<f64, MAX_STAMP_PIXELS> = stamp.y.iter().map(|&v| v as f64).collect();
    let data_z: ArrayVec<f64, MAX_STAMP_PIXELS> = stamp.z.iter().map(|&v| v as f64).collect();

    let weights = fit_weights(&data_z, background, noise);

    let initial_amplitude = (stamp.peak - background).max(0.01);

    // Estimate sigma from moments, then convert to alpha (using the fixed β).
    let sigma_est = estimate_sigma_from_moments(&stamp.x, &stamp.y, &stamp.z, pos, background);
    let fwhm_est = sigma_est * FWHM_TO_SIGMA;
    let initial_alpha =
        fwhm_beta_to_alpha(fwhm_est, config.fixed_beta).clamp(0.5, stamp_radius as f32);

    let initial_params: [f64; 5] = [
        pos.x as f64,
        pos.y as f64,
        initial_amplitude as f64,
        initial_alpha as f64,
        background as f64,
    ];

    let model = MoffatFixedBeta::new(stamp_radius as f64, config.fixed_beta as f64);

    let data = FitData::new(&data_x, &data_y, &data_z, weights.as_deref());
    let result = optimize(&model, data, initial_params, &config.lm);

    let [x0, y0, amplitude, alpha, bg] = result.params;
    let result_pos = Vec2::new(x0 as f32, y0 as f32);

    if !validate_position(result_pos, pos, alpha as f32, stamp_radius) {
        return None;
    }

    let fwhm = alpha_beta_to_fwhm(alpha as f32, config.fixed_beta);

    Some(MoffatFitResult {
        pos: result_pos,
        fwhm,
        converged: result.converged,
        debug: MoffatFitDebug {
            amplitude: amplitude as f32,
            alpha: alpha as f32,
            background: bg as f32,
            iterations: result.iterations,
        },
    })
}

fn validate_position(result_pos: Vec2, input_pos: Vec2, alpha: f32, stamp_radius: usize) -> bool {
    if (result_pos - input_pos).abs().max_element() > stamp_radius as f32 {
        return false;
    }
    if alpha < 0.5 || alpha > stamp_radius as f32 * 2.0 {
        return false;
    }
    true
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
    use crate::stacking::star_detection::centroid::moffat_fit::{
        MoffatFitResult, MoffatFixedBeta, fast_pow_neg,
    };

    impl MoffatFitResult {
        /// Exposes `MoffatFitDebug::alpha` to `centroid::tests`, which sits outside
        /// `moffat_fit` and so can't reach the private `debug` field directly.
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
