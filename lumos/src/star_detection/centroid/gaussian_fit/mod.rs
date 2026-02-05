//! 2D Gaussian fitting for high-precision centroid computation.
//!
//! Implements Levenberg-Marquardt optimization to fit a 2D Gaussian model:
//! f(x,y) = A × exp(-((x-x₀)²/2σ_x² + (y-y₀)²/2σ_y²)) + B
//!
//! Uses f64 throughout the fitting pipeline for numerical stability,
//! achieving ~0.01 pixel centroid accuracy.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use super::linear_solver::solve;
use super::lm_optimizer::{LMConfig, LMResult, compute_hessian_gradient};
use super::{estimate_sigma_from_moments, extract_stamp};
use crate::common::Buffer2;
use glam::Vec2;

/// Configuration for Gaussian fitting.
pub type GaussianFitConfig = LMConfig;

/// Result of 2D Gaussian fitting.
#[derive(Debug, Clone, Copy)]
pub struct GaussianFitResult {
    /// Position of Gaussian center (sub-pixel).
    pub pos: Vec2,
    /// Amplitude of Gaussian.
    pub amplitude: f32,
    /// Sigma in X and Y directions.
    pub sigma: Vec2,
    /// Background level.
    pub background: f32,
    /// RMS residual of fit.
    pub rms_residual: f32,
    /// Whether the fit converged.
    pub converged: bool,
    /// Number of iterations used.
    pub iterations: usize,
}

/// Fit a 2D Gaussian to a star stamp.
///
/// Uses Levenberg-Marquardt optimization to find the best-fit Gaussian
/// parameters, achieving ~0.01 pixel centroid accuracy.
/// All fitting is done in f64 for numerical stability.
pub fn fit_gaussian_2d(
    pixels: &Buffer2<f32>,
    pos: Vec2,
    stamp_radius: usize,
    background: f32,
    config: &GaussianFitConfig,
) -> Option<GaussianFitResult> {
    let (data_x_f32, data_y_f32, data_z_f32, peak_value) =
        extract_stamp(pixels, pos, stamp_radius)?;

    let n = data_x_f32.len();
    if n < 7 {
        return None;
    }

    // Convert stamp data to f64 for fitting
    let data_x: Vec<f64> = data_x_f32.iter().map(|&v| v as f64).collect();
    let data_y: Vec<f64> = data_y_f32.iter().map(|&v| v as f64).collect();
    let data_z: Vec<f64> = data_z_f32.iter().map(|&v| v as f64).collect();

    // Estimate sigma from moments for better initial guess
    let sigma_est =
        estimate_sigma_from_moments(&data_x_f32, &data_y_f32, &data_z_f32, pos, background);

    let initial_params: [f64; 6] = [
        pos.x as f64,
        pos.y as f64,
        (peak_value - background).max(0.01) as f64,
        sigma_est as f64,
        sigma_est as f64,
        background as f64,
    ];

    let result = optimize_gaussian(
        &data_x,
        &data_y,
        &data_z,
        initial_params,
        stamp_radius as f64,
        config,
    );

    validate_result(&result, pos, stamp_radius, n)
}

fn validate_result(
    result: &LMResult<6>,
    pos: Vec2,
    stamp_radius: usize,
    n: usize,
) -> Option<GaussianFitResult> {
    let [x0, y0, amplitude, sigma_x, sigma_y, bg] = result.params;

    // Check if center is within stamp
    let result_pos = Vec2::new(x0 as f32, y0 as f32);
    if (result_pos - pos).abs().max_element() > stamp_radius as f32 {
        return None;
    }

    // Check for reasonable sigma values
    if sigma_x < 0.5
        || sigma_y < 0.5
        || sigma_x > stamp_radius as f64 * 2.0
        || sigma_y > stamp_radius as f64 * 2.0
    {
        return None;
    }

    let rms = (result.chi2 / n as f64).sqrt() as f32;

    Some(GaussianFitResult {
        pos: Vec2::new(x0 as f32, y0 as f32),
        amplitude: amplitude as f32,
        sigma: Vec2::new(sigma_x as f32, sigma_y as f32),
        background: bg as f32,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
    })
}

// ============================================================================
// Gaussian model evaluation and Jacobian
// ============================================================================

/// Compute Jacobian rows and residuals for 2D Gaussian model.
fn fill_jacobian_residuals(
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    params: &[f64; 6],
    jacobian: &mut Vec<[f64; 6]>,
    residuals: &mut Vec<f64>,
) {
    let n = data_x.len();
    jacobian.clear();
    residuals.clear();
    jacobian.reserve(n);
    residuals.reserve(n);

    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;

    for i in 0..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let dx2 = dx * dx;
        let dy2 = dy * dy;
        let exponent = -0.5 * (dx2 / sigma_x2 + dy2 / sigma_y2);
        let exp_val = exponent.exp();
        let amp_exp = amp * exp_val;
        let model = amp_exp + bg;

        residuals.push(z - model);

        jacobian.push([
            amp_exp * dx / sigma_x2,              // df/dx0
            amp_exp * dy / sigma_y2,              // df/dy0
            exp_val,                              // df/damp
            amp_exp * dx2 / (sigma_x2 * sigma_x), // df/dsigma_x
            amp_exp * dy2 / (sigma_y2 * sigma_y), // df/dsigma_y
            1.0,                                  // df/dbg
        ]);
    }
}

/// Compute chi² for 2D Gaussian model.
fn compute_chi2(data_x: &[f64], data_y: &[f64], data_z: &[f64], params: &[f64; 6]) -> f64 {
    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;
    let mut chi2 = 0.0f64;

    for i in 0..data_x.len() {
        let dx = data_x[i] - x0;
        let dy = data_y[i] - y0;
        let exponent = -0.5 * (dx * dx / sigma_x2 + dy * dy / sigma_y2);
        let model = amp * exponent.exp() + bg;
        let residual = data_z[i] - model;
        chi2 += residual * residual;
    }

    chi2
}

// ============================================================================
// L-M optimizer for Gaussian fitting
// ============================================================================

/// L-M optimizer for Gaussian fitting using f64 arithmetic.
fn optimize_gaussian(
    data_x: &[f64],
    data_y: &[f64],
    data_z: &[f64],
    initial_params: [f64; 6],
    stamp_radius: f64,
    config: &LMConfig,
) -> LMResult<6> {
    let mut params = initial_params;
    let mut lambda = config.initial_lambda;
    let mut prev_chi2 = compute_chi2(data_x, data_y, data_z, &params);
    let mut converged = false;
    let mut iterations = 0;

    let n = data_x.len();
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        fill_jacobian_residuals(
            data_x,
            data_y,
            data_z,
            &params,
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
        new_params[3] = new_params[3].clamp(0.5, stamp_radius); // Sigma_x
        new_params[4] = new_params[4].clamp(0.5, stamp_radius); // Sigma_y

        let new_chi2 = compute_chi2(data_x, data_y, data_z, &new_params);

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
