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

use super::lm_optimizer::{LMConfig, LMModel, LMResult, optimize};
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

/// 2D Gaussian model for L-M optimization (6 parameters).
/// Parameters: [x0, y0, amplitude, sigma_x, sigma_y, background]
#[derive(Debug)]
pub(crate) struct Gaussian2D {
    pub stamp_radius: f64,
}

impl LMModel<6> for Gaussian2D {
    #[inline]
    fn evaluate(&self, x: f64, y: f64, params: &[f64; 6]) -> f64 {
        let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / (sigma_x * sigma_x) + dy * dy / (sigma_y * sigma_y));
        amp * exponent.exp() + bg
    }

    #[inline]
    fn jacobian_row(&self, x: f64, y: f64, params: &[f64; 6]) -> [f64; 6] {
        let [x0, y0, amp, sigma_x, sigma_y, _bg] = *params;
        let sigma_x2 = sigma_x * sigma_x;
        let sigma_y2 = sigma_y * sigma_y;
        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / sigma_x2 + dy * dy / sigma_y2);
        let exp_val = exponent.exp();
        let amp_exp = amp * exp_val;

        [
            amp_exp * dx / sigma_x2,                  // df/dx0
            amp_exp * dy / sigma_y2,                  // df/dy0
            exp_val,                                  // df/damp
            amp_exp * dx * dx / (sigma_x2 * sigma_x), // df/dsigma_x
            amp_exp * dy * dy / (sigma_y2 * sigma_y), // df/dsigma_y
            1.0,                                      // df/dbg
        ]
    }

    #[inline]
    fn constrain(&self, params: &mut [f64; 6]) {
        params[2] = params[2].max(0.01); // Amplitude > 0
        params[3] = params[3].clamp(0.5, self.stamp_radius); // Sigma_x
        params[4] = params[4].clamp(0.5, self.stamp_radius); // Sigma_y
    }
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

    let model = Gaussian2D {
        stamp_radius: stamp_radius as f64,
    };

    let result = optimize(&model, &data_x, &data_y, &data_z, initial_params, config);

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
