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

use super::linear_solver::solve;
use super::lm_optimizer::{LMConfig, LMModel, LMResult, compute_hessian_gradient, optimize_6};
use super::{estimate_sigma_from_moments, extract_stamp};
use crate::common::Buffer2;
use crate::math::FWHM_TO_SIGMA;
use glam::Vec2;

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
    pub pos: Vec2,
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

/// Moffat model with variable beta (6 parameters).
/// Parameters: [x0, y0, amplitude, alpha, beta, background]
pub(crate) struct MoffatVariableBeta {
    pub stamp_radius: f64,
}

impl LMModel<6> for MoffatVariableBeta {
    #[inline]
    fn evaluate(&self, x: f64, y: f64, params: &[f64; 6]) -> f64 {
        let [x0, y0, amp, alpha, beta, bg] = *params;
        let r2 = (x - x0).powi(2) + (y - y0).powi(2);
        amp * (1.0 + r2 / (alpha * alpha)).powf(-beta) + bg
    }

    #[inline]
    fn jacobian_row(&self, x: f64, y: f64, params: &[f64; 6]) -> [f64; 6] {
        let [x0, y0, amp, alpha, beta, _bg] = *params;
        let alpha2 = alpha * alpha;
        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let ln_u = u.ln();
        let u_neg_beta = (-beta * ln_u).exp();
        let u_neg_beta_m1 = u_neg_beta / u;
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
    fn constrain(&self, params: &mut [f64; 6]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Alpha
        params[4] = params[4].clamp(1.5, 10.0); // Beta
    }
}

/// Fit a 2D Moffat profile to a star stamp.
/// All fitting is done in f64 for numerical stability.
pub fn fit_moffat_2d(
    pixels: &Buffer2<f32>,
    pos: Vec2,
    stamp_radius: usize,
    background: f32,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let (data_x_f32, data_y_f32, data_z_f32, peak_value) =
        extract_stamp(pixels, pos, stamp_radius)?;

    let n = data_x_f32.len();
    let n_params = if config.fit_beta { 6 } else { 5 };
    if n < n_params + 1 {
        return None;
    }

    // Convert stamp data to f64 for fitting
    let data_x: Vec<f64> = data_x_f32.iter().map(|&v| v as f64).collect();
    let data_y: Vec<f64> = data_y_f32.iter().map(|&v| v as f64).collect();
    let data_z: Vec<f64> = data_z_f32.iter().map(|&v| v as f64).collect();

    let initial_amplitude = (peak_value - background).max(0.01);

    // Estimate sigma from moments, then convert to alpha
    let sigma_est =
        estimate_sigma_from_moments(&data_x_f32, &data_y_f32, &data_z_f32, pos, background);
    let fwhm_est = sigma_est * FWHM_TO_SIGMA;
    let initial_alpha =
        fwhm_beta_to_alpha(fwhm_est, config.fixed_beta).clamp(0.5, stamp_radius as f32);

    if config.fit_beta {
        fit_with_variable_beta(
            &data_x,
            &data_y,
            &data_z,
            pos,
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
            pos,
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
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    pos: Vec2,
    initial_amplitude: f32,
    initial_alpha: f32,
    background: f32,
    stamp_radius: usize,
    n: usize,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let initial_params: [f64; 5] = [
        pos.x as f64,
        pos.y as f64,
        initial_amplitude as f64,
        initial_alpha as f64,
        background as f64,
    ];

    let result = optimize_moffat_fixed_beta(
        data_x,
        data_y,
        data_z,
        initial_params,
        config.fixed_beta as f64,
        stamp_radius as f64,
        &config.lm,
    );

    let [x0, y0, amplitude, alpha, bg] = result.params;
    let result_pos = Vec2::new(x0 as f32, y0 as f32);

    if !validate_position(result_pos, pos, alpha as f32, stamp_radius) {
        return None;
    }

    let rms = (result.chi2 / n as f64).sqrt() as f32;
    let fwhm = alpha_beta_to_fwhm(alpha as f32, config.fixed_beta);

    Some(MoffatFitResult {
        pos: result_pos,
        amplitude: amplitude as f32,
        alpha: alpha as f32,
        beta: config.fixed_beta,
        background: bg as f32,
        fwhm,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
    })
}

#[allow(clippy::too_many_arguments)]
fn fit_with_variable_beta(
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    pos: Vec2,
    initial_amplitude: f32,
    initial_alpha: f32,
    background: f32,
    stamp_radius: usize,
    n: usize,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    let initial_params: [f64; 6] = [
        pos.x as f64,
        pos.y as f64,
        initial_amplitude as f64,
        initial_alpha as f64,
        config.fixed_beta as f64,
        background as f64,
    ];
    let model = MoffatVariableBeta {
        stamp_radius: stamp_radius as f64,
    };

    let result = optimize_6(&model, data_x, data_y, data_z, initial_params, &config.lm);

    let [x0, y0, amplitude, alpha, beta, bg] = result.params;
    let result_pos = Vec2::new(x0 as f32, y0 as f32);

    if !validate_position(result_pos, pos, alpha as f32, stamp_radius) {
        return None;
    }

    let rms = (result.chi2 / n as f64).sqrt() as f32;
    let fwhm = alpha_beta_to_fwhm(alpha as f32, beta as f32);

    Some(MoffatFitResult {
        pos: result_pos,
        amplitude: amplitude as f32,
        alpha: alpha as f32,
        beta: beta as f32,
        background: bg as f32,
        fwhm,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
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
pub fn alpha_beta_to_fwhm(alpha: f32, beta: f32) -> f32 {
    2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt()
}

/// Convert FWHM and beta to Moffat alpha.
/// alpha = FWHM / (2 * sqrt(2^(1/beta) - 1))
#[inline]
pub fn fwhm_beta_to_alpha(fwhm: f32, beta: f32) -> f32 {
    fwhm / (2.0 * (2.0f32.powf(1.0 / beta) - 1.0).sqrt())
}

// ============================================================================
// Fixed-beta Moffat model evaluation and Jacobian (5 parameters)
// ============================================================================

/// Compute Jacobian rows and residuals for fixed-beta Moffat model.
fn fill_jacobian_residuals_fixed_beta(
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 5],
    beta: f64,
    jacobian: &mut Vec<[f64; 5]>,
    residuals: &mut Vec<f64>,
) {
    let n = data_x.len();
    jacobian.clear();
    residuals.clear();
    jacobian.reserve(n);
    residuals.reserve(n);

    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let neg_beta = -beta;

    for i in 0..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let u_neg_beta = u.powf(neg_beta);
        let u_neg_beta_m1 = u_neg_beta / u;
        let model = amp * u_neg_beta + bg;

        residuals.push(z - model);

        let common = 2.0 * amp * beta / alpha2 * u_neg_beta_m1;
        jacobian.push([
            common * dx,
            common * dy,
            u_neg_beta,
            common * r2 / alpha,
            1.0,
        ]);
    }
}

/// Compute chi² for fixed-beta Moffat model.
fn compute_chi2_fixed_beta(
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 5],
    beta: f64,
) -> f64 {
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let mut chi2 = 0.0f64;

    for i in 0..data_x.len() {
        let dx = data_x[i] - x0;
        let dy = data_y[i] - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let model = amp * u.powf(-beta) + bg;
        let residual = data_z[i] - model;
        chi2 += residual * residual;
    }

    chi2
}

// ============================================================================
// L-M optimizer for fixed-beta Moffat
// ============================================================================

/// L-M optimizer for Moffat with fixed beta using f64 arithmetic.
fn optimize_moffat_fixed_beta(
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    initial_params: [f64; 5],
    beta: f64,
    stamp_radius: f64,
    config: &LMConfig,
) -> LMResult<5> {
    let mut params = initial_params;
    let mut lambda = config.initial_lambda;
    let mut prev_chi2 = compute_chi2_fixed_beta(data_x, data_y, data_z, &params, beta);
    let mut converged = false;
    let mut iterations = 0;

    let n = data_x.len();
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        fill_jacobian_residuals_fixed_beta(
            data_x,
            data_y,
            data_z,
            &params,
            beta,
            &mut jacobian,
            &mut residuals,
        );

        let (hessian, gradient) = compute_hessian_gradient(&jacobian, &residuals);

        let mut damped_hessian = hessian;
        for (i, row) in damped_hessian.iter_mut().enumerate() {
            row[i] *= 1.0 + lambda;
        }

        let Some(delta) = solve(&damped_hessian, &gradient) else {
            break;
        };

        let mut new_params = params;
        for (p, d) in new_params.iter_mut().zip(delta.iter()) {
            *p += d;
        }
        // Apply constraints
        new_params[2] = new_params[2].max(0.01); // Amplitude > 0
        new_params[3] = new_params[3].clamp(0.5, stamp_radius); // Alpha

        let new_chi2 = compute_chi2_fixed_beta(data_x, data_y, data_z, &new_params, beta);

        if new_chi2 < prev_chi2 {
            params = new_params;
            lambda *= config.lambda_down;

            let chi2_rel_change = (prev_chi2 - new_chi2) / prev_chi2.max(1e-30);
            prev_chi2 = new_chi2;

            let max_delta = delta.iter().copied().fold(0.0f64, |a, d| a.max(d.abs()));
            if max_delta < config.convergence_threshold || chi2_rel_change < 1e-10 {
                converged = true;
                break;
            }
            // Early exit when only position accuracy matters
            if delta[0].abs() < config.position_convergence_threshold
                && delta[1].abs() < config.position_convergence_threshold
            {
                converged = true;
                break;
            }
        } else {
            let chi2_rel_diff = (new_chi2 - prev_chi2) / prev_chi2.max(1e-30);
            if chi2_rel_diff < 1e-10 {
                converged = true;
                break;
            }

            lambda *= config.lambda_up;
            if lambda > 1e10 {
                break;
            }
        }
    }

    LMResult {
        params,
        chi2: prev_chi2,
        converged,
        iterations,
    }
}
