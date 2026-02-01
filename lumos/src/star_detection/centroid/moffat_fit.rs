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

use super::lm_optimizer::{
    LMConfig, LMModel, optimize_5, optimize_5_weighted, optimize_6, optimize_6_weighted,
};
use super::{compute_pixel_weights, estimate_sigma_from_moments, extract_stamp};
use crate::common::Buffer2;
use crate::math::FWHM_TO_SIGMA;

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
    /// X coordinate of profile center (sub-pixel).
    pub x: f32,
    /// Y coordinate of profile center (sub-pixel).
    pub y: f32,
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
struct MoffatFixedBeta {
    beta: f32,
    stamp_radius: f32,
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
struct MoffatVariableBeta {
    stamp_radius: f32,
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
    let model = MoffatFixedBeta {
        beta: config.fixed_beta,
        stamp_radius: stamp_radius as f32,
    };

    let result = optimize_5(&model, data_x, data_y, data_z, initial_params, &config.lm);

    let [x0, y0, amplitude, alpha, bg] = result.params;

    if !validate_position(x0, y0, cx, cy, alpha, stamp_radius) {
        return None;
    }

    let rms = (result.chi2 / n as f32).sqrt();
    let fwhm = alpha_beta_to_fwhm(alpha, config.fixed_beta);

    Some(MoffatFitResult {
        x: x0,
        y: y0,
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
        x: x0,
        y: y0,
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
        x: x0,
        y: y0,
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
        x: x0,
        y: y0,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn make_moffat_stamp(
        width: usize,
        height: usize,
        cx: f32,
        cy: f32,
        amplitude: f32,
        alpha: f32,
        beta: f32,
        background: f32,
    ) -> Vec<f32> {
        let mut pixels = vec![background; width * height];
        for y in 0..height {
            for x in 0..width {
                let r2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
                let value = amplitude * (1.0 + r2 / (alpha * alpha)).powf(-beta);
                pixels[y * width + x] += value;
            }
        }
        pixels
    }

    #[test]
    fn test_moffat_fit_centered_fixed_beta() {
        let width = 21;
        let height = 21;
        let true_cx = 10.0;
        let true_cy = 10.0;
        let true_amp = 1.0;
        let true_alpha = 2.5;
        let true_beta = 2.5;
        let true_bg = 0.1;

        let pixels = make_moffat_stamp(
            width, height, true_cx, true_cy, true_amp, true_alpha, true_beta, true_bg,
        );
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = MoffatFitConfig {
            fit_beta: false,
            fixed_beta: true_beta,
            ..Default::default()
        };
        let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.converged);
        assert!((result.x - true_cx).abs() < 0.1);
        assert!((result.y - true_cy).abs() < 0.1);
        assert!((result.alpha - true_alpha).abs() < 0.3);
    }

    #[test]
    fn test_moffat_fit_subpixel_offset() {
        let width = 21;
        let height = 21;
        let true_cx = 10.3;
        let true_cy = 10.7;
        let true_amp = 1.0;
        let true_alpha = 2.5;
        let true_beta = 2.5;
        let true_bg = 0.1;

        let pixels = make_moffat_stamp(
            width, height, true_cx, true_cy, true_amp, true_alpha, true_beta, true_bg,
        );
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = MoffatFitConfig {
            fit_beta: false,
            fixed_beta: true_beta,
            ..Default::default()
        };
        let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.converged);
        assert!((result.x - true_cx).abs() < 0.05);
        assert!((result.y - true_cy).abs() < 0.05);
    }

    #[test]
    fn test_moffat_fit_with_beta() {
        let width = 21;
        let height = 21;
        let true_cx = 10.0;
        let true_cy = 10.0;
        let true_amp = 1.0;
        let true_alpha = 2.5;
        let true_beta = 3.5;
        let true_bg = 0.1;

        let pixels = make_moffat_stamp(
            width, height, true_cx, true_cy, true_amp, true_alpha, true_beta, true_bg,
        );
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = MoffatFitConfig {
            fit_beta: true,
            fixed_beta: 3.0,
            lm: LMConfig {
                max_iterations: 100,
                ..Default::default()
            },
        };
        let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.converged);
        assert!((result.x - true_cx).abs() < 0.1);
        assert!((result.y - true_cy).abs() < 0.1);
        assert!((result.beta - true_beta).abs() < 0.5);
    }

    #[test]
    fn test_alpha_beta_fwhm_conversion() {
        let alpha = 2.0;
        let beta = 2.5;
        let fwhm = alpha_beta_to_fwhm(alpha, beta);
        let alpha_back = fwhm_beta_to_alpha(fwhm, beta);
        assert!((alpha_back - alpha).abs() < 1e-6);
        assert!((fwhm - 2.26).abs() < 0.1);
    }

    #[test]
    fn test_moffat_fit_edge_position() {
        let width = 21;
        let height = 21;
        let pixels = vec![0.1f32; width * height];
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = MoffatFitConfig::default();
        let result = fit_moffat_2d(&pixels_buf, 2.0, 10.0, 8, 0.1, &config);
        assert!(result.is_none());
    }
}
