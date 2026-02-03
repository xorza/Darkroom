//! 2D Moffat profile fitting for high-precision centroid computation.
//!
//! The Moffat profile is a better model for stellar PSFs than Gaussian because
//! it has extended wings that match atmospheric seeing:
//!
//! f(x,y) = A × (1 + ((x-x₀)²+(y-y₀)²)/α²)^(-β) + B
//!
//! where α is the core width and β controls the wing slope (typically 2.5-4.5).
//!
//! This achieves similar centroid accuracy to Gaussian fitting (~0.01 pixel)
//! but provides more accurate flux and FWHM estimates for stellar sources.

#![allow(dead_code)]

#[cfg(test)]
mod bench;
pub(crate) mod simd;
#[cfg(test)]
mod tests;

use super::linear_solver::solve_5x5;
use super::lm_optimizer::{
    LMConfig, LMModel, optimize_5, optimize_5_weighted, optimize_6, optimize_6_weighted,
};
use super::{compute_pixel_weights, estimate_sigma_from_moments, extract_stamp};
use crate::common::Buffer2;
use crate::math::FWHM_TO_SIGMA;
use glam::DVec2;

/// Configuration for Moffat profile fitting.
#[derive(Debug, Clone)]
pub struct MoffatFitConfig {
    /// L-M optimization parameters.
    pub lm: LMConfig,
    /// Whether to fit beta or fix it to a constant.
    pub fit_beta: bool,
    /// Fixed beta value when fit_beta is false.
    pub fixed_beta: f32,
}

impl Default for MoffatFitConfig {
    fn default() -> Self {
        Self {
            lm: LMConfig::default(),
            fit_beta: false,
            fixed_beta: 2.5,
        }
    }
}

/// Result of 2D Moffat profile fitting.
#[derive(Debug, Clone, Copy)]
pub struct MoffatFitResult {
    /// Position of profile center (sub-pixel).
    pub pos: DVec2,
    /// Amplitude of profile.
    pub amplitude: f32,
    /// Core width parameter (alpha).
    pub alpha: f32,
    /// Power law slope (beta).
    pub beta: f32,
    /// Background level.
    pub background: f32,
    /// FWHM computed from alpha and beta.
    pub fwhm: f32,
    /// RMS residual of fit.
    pub rms_residual: f32,
    /// Whether the fit converged.
    pub converged: bool,
    /// Number of iterations used.
    pub iterations: usize,
}

/// Moffat model with fixed beta (5 parameters).
/// Parameters: [x0, y0, amplitude, alpha, background]
pub(crate) struct MoffatFixedBeta {
    pub beta: f32,
    pub stamp_radius: f32,
}

impl LMModel<5> for MoffatFixedBeta {
    #[inline]
    fn evaluate(&self, x: f32, y: f32, params: &[f32; 5]) -> f32 {
        let [x0, y0, amp, alpha, bg] = *params;
        let r2 = (x - x0).powi(2) + (y - y0).powi(2);
        amp * (1.0 + r2 / (alpha * alpha)).powf(-self.beta) + bg
    }

    #[inline]
    fn jacobian_row(&self, x: f32, y: f32, params: &[f32; 5]) -> [f32; 5] {
        let [x0, y0, amp, alpha, _bg] = *params;
        let alpha2 = alpha * alpha;
        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        // Cache power computation: compute u^(-beta) once, derive u^(-beta-1) from it
        let u_neg_beta = u.powf(-self.beta);
        let u_neg_beta_m1 = u_neg_beta / u; // u^(-beta-1) = u^(-beta) / u
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
    fn constrain(&self, params: &mut [f32; 5]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Alpha
    }
}

/// Moffat model with variable beta (6 parameters).
/// Parameters: [x0, y0, amplitude, alpha, beta, background]
pub(crate) struct MoffatVariableBeta {
    pub stamp_radius: f32,
}

impl LMModel<6> for MoffatVariableBeta {
    #[inline]
    fn evaluate(&self, x: f32, y: f32, params: &[f32; 6]) -> f32 {
        let [x0, y0, amp, alpha, beta, bg] = *params;
        let r2 = (x - x0).powi(2) + (y - y0).powi(2);
        amp * (1.0 + r2 / (alpha * alpha)).powf(-beta) + bg
    }

    #[inline]
    fn jacobian_row(&self, x: f32, y: f32, params: &[f32; 6]) -> [f32; 6] {
        let [x0, y0, amp, alpha, beta, _bg] = *params;
        let alpha2 = alpha * alpha;
        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        // Cache power computation: compute ln(u) and u^(-beta) once
        let ln_u = u.ln();
        let u_neg_beta = (-beta * ln_u).exp(); // u^(-beta) = exp(-beta * ln(u))
        let u_neg_beta_m1 = u_neg_beta / u; // u^(-beta-1) = u^(-beta) / u
        let common = 2.0 * amp * beta / alpha2 * u_neg_beta_m1;

        [
            common * dx,              // df/dx0
            common * dy,              // df/dy0
            u_neg_beta,               // df/damp
            common * r2 / alpha,      // df/dalpha
            -amp * ln_u * u_neg_beta, // df/dbeta
            1.0,                      // df/dbg
        ]
    }

    #[inline]
    fn constrain(&self, params: &mut [f32; 6]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Alpha
        params[4] = params[4].clamp(1.5, 10.0); // Beta
    }
}

/// Fit a 2D Moffat profile to a star stamp.
pub fn fit_moffat_2d(
    pixels: &Buffer2<f32>,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    background: f32,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let (data_x, data_y, data_z, peak_value) = extract_stamp(pixels, cx, cy, stamp_radius)?;

    let n = data_x.len();
    let n_params = if config.fit_beta { 6 } else { 5 };
    if n < n_params + 1 {
        return None;
    }

    let initial_amplitude = (peak_value - background).max(0.01);

    // Estimate sigma from moments, then convert to alpha
    // For Moffat: FWHM ≈ FWHM_TO_SIGMA*sigma, so use fwhm_beta_to_alpha
    let sigma_est = estimate_sigma_from_moments(&data_x, &data_y, &data_z, cx, cy, background);
    let fwhm_est = sigma_est * FWHM_TO_SIGMA;
    let initial_alpha =
        fwhm_beta_to_alpha(fwhm_est, config.fixed_beta).clamp(0.5, stamp_radius as f32);

    if config.fit_beta {
        fit_with_variable_beta(
            &data_x,
            &data_y,
            &data_z,
            cx,
            cy,
            initial_amplitude,
            initial_alpha,
            background,
            stamp_radius,
            n,
            config,
        )
    } else {
        fit_with_fixed_beta(
            &data_x,
            &data_y,
            &data_z,
            cx,
            cy,
            initial_amplitude,
            initial_alpha,
            background,
            stamp_radius,
            n,
            config,
        )
    }
}

/// Fit a 2D Moffat profile to a star stamp with inverse-variance weighting.
///
/// Uses weighted Levenberg-Marquardt optimization for optimal estimation
/// when noise characteristics are known.
#[allow(clippy::too_many_arguments)]
pub fn fit_moffat_2d_weighted(
    pixels: &Buffer2<f32>,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    background: f32,
    noise: f32,
    gain: Option<f32>,
    read_noise: Option<f32>,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let (data_x, data_y, data_z, peak_value) = extract_stamp(pixels, cx, cy, stamp_radius)?;

    let n = data_x.len();
    let n_params = if config.fit_beta { 6 } else { 5 };
    if n < n_params + 1 {
        return None;
    }

    // Compute inverse-variance weights
    let weights = compute_pixel_weights(&data_z, background, noise, gain, read_noise);

    let initial_amplitude = (peak_value - background).max(0.01);

    // Estimate sigma from moments, then convert to alpha
    let sigma_est = estimate_sigma_from_moments(&data_x, &data_y, &data_z, cx, cy, background);
    let fwhm_est = sigma_est * FWHM_TO_SIGMA;
    let initial_alpha =
        fwhm_beta_to_alpha(fwhm_est, config.fixed_beta).clamp(0.5, stamp_radius as f32);

    if config.fit_beta {
        fit_with_variable_beta_weighted(
            &data_x,
            &data_y,
            &data_z,
            &weights,
            cx,
            cy,
            initial_amplitude,
            initial_alpha,
            background,
            stamp_radius,
            n,
            config,
        )
    } else {
        fit_with_fixed_beta_weighted(
            &data_x,
            &data_y,
            &data_z,
            &weights,
            cx,
            cy,
            initial_amplitude,
            initial_alpha,
            background,
            stamp_radius,
            n,
            config,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn fit_with_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    cx: f32,
    cy: f32,
    initial_amplitude: f32,
    initial_alpha: f32,
    background: f32,
    stamp_radius: usize,
    n: usize,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let initial_params = [cx, cy, initial_amplitude, initial_alpha, background];

    // Use SIMD-optimized optimizer when available
    let result = optimize_moffat_fixed_beta_simd(
        data_x,
        data_y,
        data_z,
        initial_params,
        config.fixed_beta,
        stamp_radius as f32,
        &config.lm,
    );

    let [x0, y0, amplitude, alpha, bg] = result.params;

    if !validate_position(x0, y0, cx, cy, alpha, stamp_radius) {
        return None;
    }

    let rms = (result.chi2 / n as f32).sqrt();
    let fwhm = alpha_beta_to_fwhm(alpha, config.fixed_beta);

    Some(MoffatFitResult {
        pos: DVec2::new(x0 as f64, y0 as f64),
        amplitude,
        alpha,
        beta: config.fixed_beta,
        background: bg,
        fwhm,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
    })
}

#[allow(clippy::too_many_arguments)]
fn fit_with_variable_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    cx: f32,
    cy: f32,
    initial_amplitude: f32,
    initial_alpha: f32,
    background: f32,
    stamp_radius: usize,
    n: usize,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let initial_params = [
        cx,
        cy,
        initial_amplitude,
        initial_alpha,
        config.fixed_beta,
        background,
    ];
    let model = MoffatVariableBeta {
        stamp_radius: stamp_radius as f32,
    };

    let result = optimize_6(&model, data_x, data_y, data_z, initial_params, &config.lm);

    let [x0, y0, amplitude, alpha, beta, bg] = result.params;

    if !validate_position(x0, y0, cx, cy, alpha, stamp_radius) {
        return None;
    }

    let rms = (result.chi2 / n as f32).sqrt();
    let fwhm = alpha_beta_to_fwhm(alpha, beta);

    Some(MoffatFitResult {
        pos: DVec2::new(x0 as f64, y0 as f64),
        amplitude,
        alpha,
        beta,
        background: bg,
        fwhm,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
    })
}

#[allow(clippy::too_many_arguments)]
fn fit_with_fixed_beta_weighted(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    weights: &[f32],
    cx: f32,
    cy: f32,
    initial_amplitude: f32,
    initial_alpha: f32,
    background: f32,
    stamp_radius: usize,
    n: usize,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let initial_params = [cx, cy, initial_amplitude, initial_alpha, background];
    let model = MoffatFixedBeta {
        beta: config.fixed_beta,
        stamp_radius: stamp_radius as f32,
    };

    let result = optimize_5_weighted(
        &model,
        data_x,
        data_y,
        data_z,
        weights,
        initial_params,
        &config.lm,
    );

    let [x0, y0, amplitude, alpha, bg] = result.params;

    if !validate_position(x0, y0, cx, cy, alpha, stamp_radius) {
        return None;
    }

    let rms = (result.chi2 / n as f32).sqrt();
    let fwhm = alpha_beta_to_fwhm(alpha, config.fixed_beta);

    Some(MoffatFitResult {
        pos: DVec2::new(x0 as f64, y0 as f64),
        amplitude,
        alpha,
        beta: config.fixed_beta,
        background: bg,
        fwhm,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
    })
}

#[allow(clippy::too_many_arguments)]
fn fit_with_variable_beta_weighted(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    weights: &[f32],
    cx: f32,
    cy: f32,
    initial_amplitude: f32,
    initial_alpha: f32,
    background: f32,
    stamp_radius: usize,
    n: usize,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let initial_params = [
        cx,
        cy,
        initial_amplitude,
        initial_alpha,
        config.fixed_beta,
        background,
    ];
    let model = MoffatVariableBeta {
        stamp_radius: stamp_radius as f32,
    };

    let result = optimize_6_weighted(
        &model,
        data_x,
        data_y,
        data_z,
        weights,
        initial_params,
        &config.lm,
    );

    let [x0, y0, amplitude, alpha, beta, bg] = result.params;

    if !validate_position(x0, y0, cx, cy, alpha, stamp_radius) {
        return None;
    }

    let rms = (result.chi2 / n as f32).sqrt();
    let fwhm = alpha_beta_to_fwhm(alpha, beta);

    Some(MoffatFitResult {
        pos: DVec2::new(x0 as f64, y0 as f64),
        amplitude,
        alpha,
        beta,
        background: bg,
        fwhm,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
    })
}

fn validate_position(x0: f32, y0: f32, cx: f32, cy: f32, alpha: f32, stamp_radius: usize) -> bool {
    if (x0 - cx).abs() > stamp_radius as f32 || (y0 - cy).abs() > stamp_radius as f32 {
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
pub fn alpha_beta_to_fwhm(alpha: f32, beta: f32) -> f32 {
    2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt()
}

/// Convert FWHM and beta to Moffat alpha.
/// alpha = FWHM / (2 * sqrt(2^(1/beta) - 1))
#[inline]
pub fn fwhm_beta_to_alpha(fwhm: f32, beta: f32) -> f32 {
    fwhm / (2.0 * (2.0f32.powf(1.0 / beta) - 1.0).sqrt())
}

/// SIMD-optimized L-M optimizer for Moffat with fixed beta.
///
/// Uses AVX2 SIMD when available for faster Jacobian/residual computation.
/// Falls back to scalar code when AVX2 is not available.
pub(crate) fn optimize_moffat_fixed_beta_simd(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    initial_params: [f32; 5],
    beta: f32,
    stamp_radius: f32,
    config: &LMConfig,
) -> super::lm_optimizer::LMResult<5> {
    #[cfg(target_arch = "x86_64")]
    if common::cpu_features::has_avx2_fma() {
        // SAFETY: We checked that AVX2 is available
        return unsafe {
            optimize_moffat_fixed_beta_avx2(
                data_x,
                data_y,
                data_z,
                initial_params,
                beta,
                stamp_radius,
                config,
            )
        };
    }

    // Fallback to generic optimizer
    let model = MoffatFixedBeta { beta, stamp_radius };
    optimize_5(&model, data_x, data_y, data_z, initial_params, config)
}

/// AVX2-accelerated L-M optimization for Moffat with fixed beta.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn optimize_moffat_fixed_beta_avx2(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    initial_params: [f32; 5],
    beta: f32,
    stamp_radius: f32,
    config: &LMConfig,
) -> super::lm_optimizer::LMResult<5> {
    let mut params = initial_params;
    let mut lambda = config.initial_lambda;
    let mut prev_chi2 = simd::compute_chi2_simd_fixed_beta(data_x, data_y, data_z, &params, beta);
    let mut converged = false;
    let mut iterations = 0;

    // Pre-allocate buffers
    let n = data_x.len();
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        // Use SIMD to fill Jacobian and residuals
        simd::fill_jacobian_residuals_simd_fixed_beta(
            data_x,
            data_y,
            data_z,
            &params,
            beta,
            &mut jacobian,
            &mut residuals,
        );

        let (hessian, gradient) = compute_hessian_gradient_5(&jacobian, &residuals);

        let mut damped_hessian = hessian;
        for (i, row) in damped_hessian.iter_mut().enumerate() {
            row[i] *= 1.0 + lambda;
        }

        let Some(delta) = solve_5x5(&damped_hessian, &gradient) else {
            break;
        };

        let mut new_params = params;
        for (p, d) in new_params.iter_mut().zip(delta.iter()) {
            *p += d;
        }
        constrain_moffat_params(&mut new_params, stamp_radius);

        let new_chi2 =
            simd::compute_chi2_simd_fixed_beta(data_x, data_y, data_z, &new_params, beta);

        if new_chi2 < prev_chi2 {
            params = new_params;
            lambda *= config.lambda_down;

            // Check for chi2 stagnation (converged to machine precision)
            let chi2_rel_change = (prev_chi2 - new_chi2) / prev_chi2.max(1e-30);
            prev_chi2 = new_chi2;

            let max_delta = delta.iter().copied().fold(0.0f32, |a, d| a.max(d.abs()));
            if max_delta < config.convergence_threshold || chi2_rel_change < 1e-6 {
                converged = true;
                break;
            }
        } else {
            // Check if chi2 is essentially the same (numerical precision limit)
            let chi2_rel_diff = (new_chi2 - prev_chi2) / prev_chi2.max(1e-30);
            if chi2_rel_diff < 1e-6 {
                // Converged to numerical precision
                converged = true;
                break;
            }

            lambda *= config.lambda_up;
            if lambda > 1e10 {
                break;
            }
        }
    }

    super::lm_optimizer::LMResult {
        params,
        chi2: prev_chi2,
        converged,
        iterations,
    }
}

/// Apply Moffat parameter constraints.
#[inline]
fn constrain_moffat_params(params: &mut [f32; 5], stamp_radius: f32) {
    params[2] = params[2].max(0.01); // Amplitude > 0
    params[3] = params[3].clamp(0.5, stamp_radius); // Alpha
}

/// Compute Hessian (J^T J) and gradient (J^T r) for 5-parameter model.
fn compute_hessian_gradient_5(
    jacobian: &[[f32; 5]],
    residuals: &[f32],
) -> ([[f32; 5]; 5], [f32; 5]) {
    let mut hessian = [[0.0f32; 5]; 5];
    let mut gradient = [0.0f32; 5];

    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..5 {
            gradient[i] += row[i] * r;
            for j in 0..5 {
                hessian[i][j] += row[i] * row[j];
            }
        }
    }

    (hessian, gradient)
}
