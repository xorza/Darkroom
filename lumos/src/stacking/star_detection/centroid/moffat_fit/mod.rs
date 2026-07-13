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

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

#[cfg(target_arch = "x86_64")]
mod simd_avx2;

#[cfg(target_arch = "aarch64")]
mod simd_neon;

use crate::math::FWHM_TO_SIGMA;
use crate::stacking::star_detection::centroid::lm_optimizer::{
    LMConfig, LMModel, accumulate_chi2, build_normal_equations_scalar, optimize,
};
use crate::stacking::star_detection::centroid::{
    FitNoise, MAX_STAMP_PIXELS, estimate_sigma_from_moments, extract_stamp, fit_weights,
};
use arrayvec::ArrayVec;
use glam::Vec2;
use imaginarium::Buffer2;

/// Configuration for Moffat profile fitting.
#[derive(Debug, Clone)]
pub(crate) struct MoffatFitConfig {
    /// L-M optimization parameters.
    pub lm: LMConfig,
    /// Fixed Moffat β (wing-slope) used for the fit.
    pub fixed_beta: f32,
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
pub(crate) struct MoffatFitResult {
    /// Position of profile center (sub-pixel).
    pub pos: Vec2,
    /// FWHM computed from alpha and beta.
    pub fwhm: f32,
    /// Whether the fit converged.
    pub converged: bool,
    /// Fit diagnostics (amplitude/alpha/background/iteration count) that no
    /// production caller reads — `measure_star` only uses `pos`/`fwhm`/`converged` —
    /// but that tests need to verify LM convergence against synthetic ground truth.
    #[allow(dead_code)] // read only by tests
    pub debug: MoffatFitDebug,
}

/// Fit diagnostics kept for tests; see [`MoffatFitResult::debug`].
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // read only by tests
pub(crate) struct MoffatFitDebug {
    /// Amplitude of profile.
    pub amplitude: f32,
    /// Core width parameter (alpha).
    pub alpha: f32,
    /// Background level.
    pub background: f32,
    /// Number of iterations used.
    pub iterations: usize,
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
pub(crate) struct MoffatFixedBeta {
    pub stamp_radius: f64,
    pub beta: f64,
    pow_strategy: PowStrategy,
}

impl MoffatFixedBeta {
    pub fn new(stamp_radius: f64, beta: f64) -> Self {
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
    fn jacobian_row(&self, x: f64, y: f64, params: &[f64; 5]) -> [f64; 5] {
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

    // Scalar fallback is dead code on aarch64, where the NEON path returns unconditionally.
    #[allow(unreachable_code)]
    fn batch_build_normal_equations(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        params: &[f64; 5],
    ) -> ([[f64; 5]; 5], [f64; 5], f64) {
        #[cfg(target_arch = "x86_64")]
        if common::cpu_features::has_avx2_fma() {
            // SAFETY: AVX2+FMA availability checked above
            return unsafe {
                simd_avx2::batch_build_normal_equations_avx2(self, data_x, data_y, data_z, params)
            };
        }
        #[cfg(target_arch = "aarch64")]
        {
            return unsafe {
                simd_neon::batch_build_normal_equations_neon(self, data_x, data_y, data_z, params)
            };
        }
        // Scalar fallback
        build_normal_equations_scalar(self, data_x, data_y, data_z, None, params)
    }

    // Scalar fallback is dead code on aarch64, where the NEON path returns unconditionally.
    #[allow(unreachable_code)]
    fn batch_compute_chi2(
        &self,
        data_x: &[f64],
        data_y: &[f64],
        data_z: &[f64],
        params: &[f64; 5],
    ) -> f64 {
        #[cfg(target_arch = "x86_64")]
        if common::cpu_features::has_avx2_fma() {
            // SAFETY: AVX2+FMA availability checked above
            return unsafe {
                simd_avx2::batch_compute_chi2_avx2(self, data_x, data_y, data_z, params)
            };
        }
        #[cfg(target_arch = "aarch64")]
        {
            return unsafe {
                simd_neon::batch_compute_chi2_neon(self, data_x, data_y, data_z, params)
            };
        }
        // Scalar fallback
        accumulate_chi2(self, data_x, data_y, data_z, None, params, 0..data_x.len())
    }
}

/// Fit a 2D Moffat profile to a star stamp via Levenberg-Marquardt (f64 throughout). When
/// `noise` is set, each pixel is weighted by `1/σ²` from the CCD noise model so the
/// shot-noisy bright core doesn't bias the fit (PR1); `None` is a plain unweighted fit.
pub(crate) fn fit_moffat_2d(
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

    let result = optimize(
        &model,
        &data_x,
        &data_y,
        &data_z,
        weights.as_deref(),
        initial_params,
        &config.lm,
    );

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
pub(crate) fn alpha_beta_to_fwhm(alpha: f32, beta: f32) -> f32 {
    2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt()
}

/// Convert FWHM and beta to Moffat alpha.
/// alpha = FWHM / (2 * sqrt(2^(1/beta) - 1))
#[inline]
pub(crate) fn fwhm_beta_to_alpha(fwhm: f32, beta: f32) -> f32 {
    fwhm / (2.0 * (2.0f32.powf(1.0 / beta) - 1.0).sqrt())
}
