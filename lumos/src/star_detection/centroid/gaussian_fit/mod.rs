//! 2D Gaussian fitting for high-precision centroid computation.
//!
//! Implements Levenberg-Marquardt optimization to fit a 2D Gaussian model:
//! f(x,y) = A × exp(-((x-x₀)²/2σ_x² + (y-y₀)²/2σ_y²)) + B
//!
//! This achieves ~0.01 pixel centroid accuracy compared to ~0.05 for weighted centroid.

#![allow(dead_code)]

#[cfg(test)]
mod bench;
pub(crate) mod simd;
#[cfg(test)]
mod tests;

use super::linear_solver::solve_6x6;
use super::lm_optimizer::{LMConfig, LMModel, LMResult, optimize_6, optimize_6_weighted};
use super::{compute_pixel_weights, estimate_sigma_from_moments, extract_stamp};
use crate::common::Buffer2;

/// Configuration for Gaussian fitting.
pub type GaussianFitConfig = LMConfig;

/// Result of 2D Gaussian fitting.
#[derive(Debug, Clone, Copy)]
pub struct GaussianFitResult {
    /// X coordinate of Gaussian center (sub-pixel).
    pub x: f32,
    /// Y coordinate of Gaussian center (sub-pixel).
    pub y: f32,
    /// Amplitude of Gaussian.
    pub amplitude: f32,
    /// Sigma in X direction.
    pub sigma_x: f32,
    /// Sigma in Y direction.
    pub sigma_y: f32,
    /// Background level.
    pub background: f32,
    /// RMS residual of fit.
    pub rms_residual: f32,
    /// Whether the fit converged.
    pub converged: bool,
    /// Number of iterations used.
    pub iterations: usize,
}

/// 2D Gaussian model for L-M fitting.
/// Parameters: [x0, y0, amplitude, sigma_x, sigma_y, background]
pub(crate) struct Gaussian2D {
    pub stamp_radius: f32,
}

impl LMModel<6> for Gaussian2D {
    #[inline]
    fn evaluate(&self, x: f32, y: f32, params: &[f32; 6]) -> f32 {
        let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / (sigma_x * sigma_x) + dy * dy / (sigma_y * sigma_y));
        amp * exponent.exp() + bg
    }

    #[inline]
    fn jacobian_row(&self, x: f32, y: f32, params: &[f32; 6]) -> [f32; 6] {
        let [x0, y0, amp, sigma_x, sigma_y, _bg] = *params;
        let sigma_x2 = sigma_x * sigma_x;
        let sigma_y2 = sigma_y * sigma_y;

        let dx = x - x0;
        let dy = y - y0;
        let dx2 = dx * dx;
        let dy2 = dy * dy;
        let exponent = -0.5 * (dx2 / sigma_x2 + dy2 / sigma_y2);
        let exp_val = exponent.exp();
        let amp_exp = amp * exp_val;

        [
            amp_exp * dx / sigma_x2,              // df/dx0
            amp_exp * dy / sigma_y2,              // df/dy0
            exp_val,                              // df/damp
            amp_exp * dx2 / (sigma_x2 * sigma_x), // df/dsigma_x
            amp_exp * dy2 / (sigma_y2 * sigma_y), // df/dsigma_y
            1.0,                                  // df/dbg
        ]
    }

    #[inline]
    fn constrain(&self, params: &mut [f32; 6]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Sigma_x
        params[4] = params[4].clamp(0.5, self.stamp_radius); // Sigma_y
    }
}

/// Fit a 2D Gaussian to a star stamp.
///
/// Uses Levenberg-Marquardt optimization to find the best-fit Gaussian
/// parameters, achieving ~0.01 pixel centroid accuracy.
pub fn fit_gaussian_2d(
    pixels: &Buffer2<f32>,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    background: f32,
    config: &GaussianFitConfig,
) -> Option<GaussianFitResult> {
    let (data_x, data_y, data_z, peak_value) = extract_stamp(pixels, cx, cy, stamp_radius)?;

    let n = data_x.len();
    if n < 7 {
        return None;
    }

    // Estimate sigma from moments for better initial guess
    let sigma_est = estimate_sigma_from_moments(&data_x, &data_y, &data_z, cx, cy, background);

    let initial_params = [
        cx,
        cy,
        (peak_value - background).max(0.01),
        sigma_est,
        sigma_est,
        background,
    ];

    // Use SIMD-optimized optimizer
    let result = optimize_gaussian_simd(
        &data_x,
        &data_y,
        &data_z,
        initial_params,
        stamp_radius as f32,
        config,
    );

    validate_result(&result, cx, cy, stamp_radius, n)
}

/// Fit a 2D Gaussian to a star stamp with inverse-variance weighting.
///
/// Uses weighted Levenberg-Marquardt optimization for optimal estimation
/// when noise characteristics are known.
#[allow(clippy::too_many_arguments)]
pub fn fit_gaussian_2d_weighted(
    pixels: &Buffer2<f32>,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    background: f32,
    noise: f32,
    gain: Option<f32>,
    read_noise: Option<f32>,
    config: &GaussianFitConfig,
) -> Option<GaussianFitResult> {
    let (data_x, data_y, data_z, peak_value) = extract_stamp(pixels, cx, cy, stamp_radius)?;

    let n = data_x.len();
    if n < 7 {
        return None;
    }

    // Compute inverse-variance weights
    let weights = compute_pixel_weights(&data_z, background, noise, gain, read_noise);

    // Estimate sigma from moments for better initial guess
    let sigma_est = estimate_sigma_from_moments(&data_x, &data_y, &data_z, cx, cy, background);

    let initial_params = [
        cx,
        cy,
        (peak_value - background).max(0.01),
        sigma_est,
        sigma_est,
        background,
    ];

    let model = Gaussian2D {
        stamp_radius: stamp_radius as f32,
    };
    let result = optimize_6_weighted(
        &model,
        &data_x,
        &data_y,
        &data_z,
        &weights,
        initial_params,
        config,
    );

    validate_result(&result, cx, cy, stamp_radius, n)
}

fn validate_result(
    result: &LMResult<6>,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    n: usize,
) -> Option<GaussianFitResult> {
    let [x0, y0, amplitude, sigma_x, sigma_y, bg] = result.params;

    // Check if center is within stamp
    if (x0 - cx).abs() > stamp_radius as f32 || (y0 - cy).abs() > stamp_radius as f32 {
        return None;
    }

    // Check for reasonable sigma values
    if sigma_x < 0.5
        || sigma_y < 0.5
        || sigma_x > stamp_radius as f32 * 2.0
        || sigma_y > stamp_radius as f32 * 2.0
    {
        return None;
    }

    let rms = (result.chi2 / n as f32).sqrt();

    Some(GaussianFitResult {
        x: x0,
        y: y0,
        amplitude,
        sigma_x,
        sigma_y,
        background: bg,
        rms_residual: rms,
        converged: result.converged,
        iterations: result.iterations,
    })
}

/// SIMD-optimized L-M optimizer for Gaussian fitting.
pub(crate) fn optimize_gaussian_simd(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    initial_params: [f32; 6],
    stamp_radius: f32,
    config: &LMConfig,
) -> LMResult<6> {
    #[cfg(target_arch = "x86_64")]
    if common::cpu_features::has_avx2_fma() {
        return unsafe {
            optimize_gaussian_avx2(data_x, data_y, data_z, initial_params, stamp_radius, config)
        };
    }

    // Fallback to generic optimizer
    let model = Gaussian2D { stamp_radius };
    optimize_6(&model, data_x, data_y, data_z, initial_params, config)
}

/// AVX2-accelerated L-M optimization for Gaussian fitting.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2", enable = "fma")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn optimize_gaussian_avx2(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    initial_params: [f32; 6],
    stamp_radius: f32,
    config: &LMConfig,
) -> LMResult<6> {
    let mut params = initial_params;
    let mut lambda = config.initial_lambda;
    let mut prev_chi2 = simd::compute_chi2_simd(data_x, data_y, data_z, &params);
    let mut converged = false;
    let mut iterations = 0;

    let n = data_x.len();
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        simd::fill_jacobian_residuals_simd(
            data_x,
            data_y,
            data_z,
            &params,
            &mut jacobian,
            &mut residuals,
        );

        let (hessian, gradient) = compute_hessian_gradient_6(&jacobian, &residuals);

        let mut damped_hessian = hessian;
        for (i, row) in damped_hessian.iter_mut().enumerate() {
            row[i] *= 1.0 + lambda;
        }

        let Some(delta) = solve_6x6(&damped_hessian, &gradient) else {
            break;
        };

        let mut new_params = params;
        for (p, d) in new_params.iter_mut().zip(delta.iter()) {
            *p += d;
        }
        constrain_gaussian_params(&mut new_params, stamp_radius);

        let new_chi2 = simd::compute_chi2_simd(data_x, data_y, data_z, &new_params);

        if new_chi2 < prev_chi2 {
            params = new_params;
            lambda *= config.lambda_down;

            let chi2_rel_change = (prev_chi2 - new_chi2) / prev_chi2.max(1e-30);
            prev_chi2 = new_chi2;

            let max_delta = delta.iter().copied().fold(0.0f32, |a, d| a.max(d.abs()));
            if max_delta < config.convergence_threshold || chi2_rel_change < 1e-6 {
                converged = true;
                break;
            }
        } else {
            let chi2_rel_diff = (new_chi2 - prev_chi2) / prev_chi2.max(1e-30);
            if chi2_rel_diff < 1e-6 {
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

#[inline]
fn constrain_gaussian_params(params: &mut [f32; 6], stamp_radius: f32) {
    params[2] = params[2].max(0.01); // Amplitude > 0
    params[3] = params[3].clamp(0.5, stamp_radius); // Sigma_x
    params[4] = params[4].clamp(0.5, stamp_radius); // Sigma_y
}

fn compute_hessian_gradient_6(
    jacobian: &[[f32; 6]],
    residuals: &[f32],
) -> ([[f32; 6]; 6], [f32; 6]) {
    let mut hessian = [[0.0f32; 6]; 6];
    let mut gradient = [0.0f32; 6];

    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..6 {
            gradient[i] += row[i] * r;
            for j in 0..6 {
                hessian[i][j] += row[i] * row[j];
            }
        }
    }

    (hessian, gradient)
}
