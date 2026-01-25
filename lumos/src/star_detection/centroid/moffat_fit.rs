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

// These are public API functions exported for external use
#![allow(dead_code)]

// Use shared linear solvers
use super::linear_solver::{solve_5x5, solve_6x6};

/// Configuration for Moffat profile fitting.
#[derive(Debug, Clone)]
pub struct MoffatFitConfig {
    /// Maximum iterations for Levenberg-Marquardt optimization.
    pub max_iterations: usize,
    /// Convergence threshold for parameter changes.
    pub convergence_threshold: f32,
    /// Initial damping parameter for L-M algorithm.
    pub initial_lambda: f32,
    /// Factor to increase lambda on failed step.
    pub lambda_up: f32,
    /// Factor to decrease lambda on successful step.
    pub lambda_down: f32,
    /// Whether to fit beta or fix it to a constant.
    pub fit_beta: bool,
    /// Fixed beta value when fit_beta is false.
    pub fixed_beta: f32,
}

impl Default for MoffatFitConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            convergence_threshold: 1e-6,
            initial_lambda: 0.001,
            lambda_up: 10.0,
            lambda_down: 0.1,
            fit_beta: false, // Fixing beta is more robust
            fixed_beta: 2.5, // Typical value for ground-based seeing
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

/// Fit a 2D Moffat profile to a star stamp.
///
/// # Arguments
/// * `pixels` - Image pixel data
/// * `width` - Image width
/// * `height` - Image height
/// * `cx` - Initial X estimate (from weighted centroid)
/// * `cy` - Initial Y estimate (from weighted centroid)
/// * `stamp_radius` - Radius of stamp to fit
/// * `background` - Background estimate
/// * `config` - Fitting configuration
///
/// # Returns
/// `Some(MoffatFitResult)` if fit succeeds, `None` if fitting fails.
#[allow(clippy::too_many_arguments)]
pub fn fit_moffat_2d(
    pixels: &[f32],
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    background: f32,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    // Extract stamp
    let icx = cx.round() as isize;
    let icy = cy.round() as isize;

    if icx < stamp_radius as isize
        || icy < stamp_radius as isize
        || icx >= (width - stamp_radius) as isize
        || icy >= (height - stamp_radius) as isize
    {
        return None;
    }

    // Collect data points
    let stamp_radius_i32 = stamp_radius as i32;
    let mut data_x = Vec::new();
    let mut data_y = Vec::new();
    let mut data_z = Vec::new();

    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        for dx in -stamp_radius_i32..=stamp_radius_i32 {
            let x = (icx + dx as isize) as usize;
            let y = (icy + dy as isize) as usize;
            let idx = y * width + x;
            let value = pixels[idx];

            data_x.push(x as f32);
            data_y.push(y as f32);
            data_z.push(value);
        }
    }

    let n = data_x.len();
    let n_params = if config.fit_beta { 6 } else { 5 };
    if n < n_params + 1 {
        return None;
    }

    // Initial parameter estimates
    let peak_value = data_z.iter().fold(f32::MIN, |a, &b| a.max(b));
    let initial_amplitude = (peak_value - background).max(0.01);
    let initial_alpha = 2.0; // Initial estimate

    if config.fit_beta {
        fit_moffat_with_beta(
            &data_x,
            &data_y,
            &data_z,
            cx,
            cy,
            initial_amplitude,
            initial_alpha,
            background,
            stamp_radius,
            config,
        )
    } else {
        fit_moffat_fixed_beta(
            &data_x,
            &data_y,
            &data_z,
            cx,
            cy,
            initial_amplitude,
            initial_alpha,
            background,
            stamp_radius,
            config,
        )
    }
}

/// Fit Moffat profile with fixed beta (5 parameters).
#[allow(clippy::too_many_arguments)]
fn fit_moffat_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    cx: f32,
    cy: f32,
    initial_amplitude: f32,
    initial_alpha: f32,
    background: f32,
    stamp_radius: usize,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    // Parameters: [x0, y0, amplitude, alpha, background]
    let mut params = [cx, cy, initial_amplitude, initial_alpha, background];
    let beta = config.fixed_beta;

    let mut lambda = config.initial_lambda;
    let mut prev_chi2 = compute_chi2_fixed_beta(data_x, data_y, data_z, &params, beta);
    let mut converged = false;
    let mut iterations = 0;

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        let (jacobian, residuals) =
            compute_jacobian_fixed_beta(data_x, data_y, data_z, &params, beta);
        let hessian = compute_hessian_5(&jacobian);
        let gradient = compute_gradient_5(&jacobian, &residuals);

        let mut damped_hessian = hessian;
        for (i, row) in damped_hessian.iter_mut().enumerate() {
            row[i] *= 1.0 + lambda;
        }

        let delta = match solve_5x5(&damped_hessian, &gradient) {
            Some(d) => d,
            None => break,
        };

        let mut new_params = params;
        for (p, d) in new_params.iter_mut().zip(delta.iter()) {
            *p += d;
        }

        // Constrain parameters
        new_params[2] = new_params[2].max(0.01); // Amplitude > 0
        new_params[3] = new_params[3].clamp(0.5, stamp_radius as f32); // Alpha

        let new_chi2 = compute_chi2_fixed_beta(data_x, data_y, data_z, &new_params, beta);

        if new_chi2 < prev_chi2 {
            params = new_params;
            lambda *= config.lambda_down;
            prev_chi2 = new_chi2;

            let max_delta = delta.iter().map(|d| d.abs()).fold(0.0f32, f32::max);
            if max_delta < config.convergence_threshold {
                converged = true;
                break;
            }
        } else {
            lambda *= config.lambda_up;
            if lambda > 1e10 {
                break;
            }
        }
    }

    let [x0, y0, amplitude, alpha, bg] = params;

    // Validate result
    if (x0 - cx).abs() > stamp_radius as f32 || (y0 - cy).abs() > stamp_radius as f32 {
        return None;
    }

    if alpha < 0.5 || alpha > stamp_radius as f32 * 2.0 {
        return None;
    }

    let n = data_x.len();
    let rms = (prev_chi2 / n as f32).sqrt();
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
        converged,
        iterations,
    })
}

/// Fit Moffat profile with variable beta (6 parameters).
#[allow(clippy::too_many_arguments)]
fn fit_moffat_with_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    cx: f32,
    cy: f32,
    initial_amplitude: f32,
    initial_alpha: f32,
    background: f32,
    stamp_radius: usize,
    config: &MoffatFitConfig,
) -> Option<MoffatFitResult> {
    // Parameters: [x0, y0, amplitude, alpha, beta, background]
    let mut params = [
        cx,
        cy,
        initial_amplitude,
        initial_alpha,
        config.fixed_beta, // Use as initial guess
        background,
    ];

    let mut lambda = config.initial_lambda;
    let mut prev_chi2 = compute_chi2_with_beta(data_x, data_y, data_z, &params);
    let mut converged = false;
    let mut iterations = 0;

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        let (jacobian, residuals) = compute_jacobian_with_beta(data_x, data_y, data_z, &params);
        let hessian = compute_hessian_6(&jacobian);
        let gradient = compute_gradient_6(&jacobian, &residuals);

        let mut damped_hessian = hessian;
        for (i, row) in damped_hessian.iter_mut().enumerate() {
            row[i] *= 1.0 + lambda;
        }

        let delta = match solve_6x6(&damped_hessian, &gradient) {
            Some(d) => d,
            None => break,
        };

        let mut new_params = params;
        for (p, d) in new_params.iter_mut().zip(delta.iter()) {
            *p += d;
        }

        // Constrain parameters
        new_params[2] = new_params[2].max(0.01); // Amplitude > 0
        new_params[3] = new_params[3].clamp(0.5, stamp_radius as f32); // Alpha
        new_params[4] = new_params[4].clamp(1.5, 10.0); // Beta typically 1.5-10

        let new_chi2 = compute_chi2_with_beta(data_x, data_y, data_z, &new_params);

        if new_chi2 < prev_chi2 {
            params = new_params;
            lambda *= config.lambda_down;
            prev_chi2 = new_chi2;

            let max_delta = delta.iter().map(|d| d.abs()).fold(0.0f32, f32::max);
            if max_delta < config.convergence_threshold {
                converged = true;
                break;
            }
        } else {
            lambda *= config.lambda_up;
            if lambda > 1e10 {
                break;
            }
        }
    }

    let [x0, y0, amplitude, alpha, beta, bg] = params;

    // Validate result
    if (x0 - cx).abs() > stamp_radius as f32 || (y0 - cy).abs() > stamp_radius as f32 {
        return None;
    }

    if alpha < 0.5 || alpha > stamp_radius as f32 * 2.0 {
        return None;
    }

    let n = data_x.len();
    let rms = (prev_chi2 / n as f32).sqrt();
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
        converged,
        iterations,
    })
}

/// Evaluate 2D Moffat profile at a point.
#[inline]
#[allow(clippy::too_many_arguments)]
fn moffat_2d(x: f32, y: f32, x0: f32, y0: f32, amp: f32, alpha: f32, beta: f32, bg: f32) -> f32 {
    let dx = x - x0;
    let dy = y - y0;
    let r2 = dx * dx + dy * dy;
    let alpha2 = alpha * alpha;
    amp * (1.0 + r2 / alpha2).powf(-beta) + bg
}

/// Compute chi-squared for fixed-beta model.
fn compute_chi2_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 5],
    beta: f32,
) -> f32 {
    let [x0, y0, amp, alpha, bg] = *params;
    data_x
        .iter()
        .zip(data_y.iter())
        .zip(data_z.iter())
        .map(|((&x, &y), &z)| {
            let model = moffat_2d(x, y, x0, y0, amp, alpha, beta, bg);
            let residual = z - model;
            residual * residual
        })
        .sum()
}

/// Compute chi-squared for variable-beta model.
fn compute_chi2_with_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 6],
) -> f32 {
    let [x0, y0, amp, alpha, beta, bg] = *params;
    data_x
        .iter()
        .zip(data_y.iter())
        .zip(data_z.iter())
        .map(|((&x, &y), &z)| {
            let model = moffat_2d(x, y, x0, y0, amp, alpha, beta, bg);
            let residual = z - model;
            residual * residual
        })
        .sum()
}

/// Compute Jacobian for fixed-beta model (5 parameters).
fn compute_jacobian_fixed_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 5],
    beta: f32,
) -> (Vec<[f32; 5]>, Vec<f32>) {
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;

    let n = data_x.len();
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for i in 0..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let u_neg_beta = u.powf(-beta);
        let u_neg_beta_m1 = u.powf(-beta - 1.0);

        let model = amp * u_neg_beta + bg;

        // Partial derivatives
        // df/dx0 = amp * (-beta) * u^(-beta-1) * (-2*dx/alpha²)
        //        = 2 * amp * beta * dx / alpha² * u^(-beta-1)
        let df_dx0 = 2.0 * amp * beta * dx / alpha2 * u_neg_beta_m1;
        // df/dy0 similarly
        let df_dy0 = 2.0 * amp * beta * dy / alpha2 * u_neg_beta_m1;
        // df/damp = u^(-beta)
        let df_damp = u_neg_beta;
        // df/dalpha = amp * (-beta) * u^(-beta-1) * (-2*r²/alpha³)
        //           = 2 * amp * beta * r² / alpha³ * u^(-beta-1)
        let df_dalpha = 2.0 * amp * beta * r2 / (alpha2 * alpha) * u_neg_beta_m1;
        // df/dbg = 1
        let df_dbg = 1.0;

        jacobian.push([df_dx0, df_dy0, df_damp, df_dalpha, df_dbg]);
        residuals.push(z - model);
    }

    (jacobian, residuals)
}

/// Compute Jacobian for variable-beta model (6 parameters).
fn compute_jacobian_with_beta(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 6],
) -> (Vec<[f32; 6]>, Vec<f32>) {
    let [x0, y0, amp, alpha, beta, bg] = *params;
    let alpha2 = alpha * alpha;

    let n = data_x.len();
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for i in 0..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let r2 = dx * dx + dy * dy;
        let u = 1.0 + r2 / alpha2;
        let u_neg_beta = u.powf(-beta);
        let u_neg_beta_m1 = u.powf(-beta - 1.0);

        let model = amp * u_neg_beta + bg;

        // Partial derivatives (same as above plus beta derivative)
        let df_dx0 = 2.0 * amp * beta * dx / alpha2 * u_neg_beta_m1;
        let df_dy0 = 2.0 * amp * beta * dy / alpha2 * u_neg_beta_m1;
        let df_damp = u_neg_beta;
        let df_dalpha = 2.0 * amp * beta * r2 / (alpha2 * alpha) * u_neg_beta_m1;
        // df/dbeta = amp * u^(-beta) * (-ln(u)) = -amp * ln(u) * u^(-beta)
        let df_dbeta = -amp * u.ln() * u_neg_beta;
        let df_dbg = 1.0;

        jacobian.push([df_dx0, df_dy0, df_damp, df_dalpha, df_dbeta, df_dbg]);
        residuals.push(z - model);
    }

    (jacobian, residuals)
}

/// Compute Hessian approximation for 5-parameter model.
fn compute_hessian_5(jacobian: &[[f32; 5]]) -> [[f32; 5]; 5] {
    let mut hessian = [[0.0f32; 5]; 5];
    for row in jacobian {
        for i in 0..5 {
            for j in 0..5 {
                hessian[i][j] += row[i] * row[j];
            }
        }
    }
    hessian
}

/// Compute Hessian approximation for 6-parameter model.
fn compute_hessian_6(jacobian: &[[f32; 6]]) -> [[f32; 6]; 6] {
    let mut hessian = [[0.0f32; 6]; 6];
    for row in jacobian {
        for i in 0..6 {
            for j in 0..6 {
                hessian[i][j] += row[i] * row[j];
            }
        }
    }
    hessian
}

/// Compute gradient for 5-parameter model.
fn compute_gradient_5(jacobian: &[[f32; 5]], residuals: &[f32]) -> [f32; 5] {
    let mut gradient = [0.0f32; 5];
    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..5 {
            gradient[i] += row[i] * r;
        }
    }
    gradient
}

/// Compute gradient for 6-parameter model.
fn compute_gradient_6(jacobian: &[[f32; 6]], residuals: &[f32]) -> [f32; 6] {
    let mut gradient = [0.0f32; 6];
    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..6 {
            gradient[i] += row[i] * r;
        }
    }
    gradient
}

/// Convert Moffat alpha and beta to FWHM.
///
/// FWHM = 2 * alpha * sqrt(2^(1/beta) - 1)
#[inline]
pub fn alpha_beta_to_fwhm(alpha: f32, beta: f32) -> f32 {
    2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt()
}

/// Convert FWHM and beta to Moffat alpha.
///
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
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let r2 = dx * dx + dy * dy;
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

        let config = MoffatFitConfig {
            fit_beta: false,
            fixed_beta: true_beta,
            ..Default::default()
        };
        let result = fit_moffat_2d(&pixels, width, height, 10.0, 10.0, 8, true_bg, &config);

        assert!(result.is_some(), "Fit should succeed");
        let result = result.unwrap();

        assert!(result.converged, "Fit should converge");
        assert!(
            (result.x - true_cx).abs() < 0.1,
            "X should be accurate: {} vs {}",
            result.x,
            true_cx
        );
        assert!(
            (result.y - true_cy).abs() < 0.1,
            "Y should be accurate: {} vs {}",
            result.y,
            true_cy
        );
        assert!(
            (result.alpha - true_alpha).abs() < 0.3,
            "Alpha should be accurate: {} vs {}",
            result.alpha,
            true_alpha
        );
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

        let config = MoffatFitConfig {
            fit_beta: false,
            fixed_beta: true_beta,
            ..Default::default()
        };
        // Start closer to true center (within 1 pixel)
        let result = fit_moffat_2d(&pixels, width, height, 10.0, 10.0, 8, true_bg, &config);

        assert!(result.is_some(), "Fit should succeed");
        let result = result.unwrap();

        assert!(result.converged, "Fit should converge");
        assert!(
            (result.x - true_cx).abs() < 0.05,
            "X should be sub-pixel accurate: {} vs {}",
            result.x,
            true_cx
        );
        assert!(
            (result.y - true_cy).abs() < 0.05,
            "Y should be sub-pixel accurate: {} vs {}",
            result.y,
            true_cy
        );
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

        let config = MoffatFitConfig {
            fit_beta: true,
            fixed_beta: 3.0,     // Start closer to true beta
            max_iterations: 100, // More iterations for beta fitting
            ..Default::default()
        };
        let result = fit_moffat_2d(&pixels, width, height, 10.0, 10.0, 8, true_bg, &config);

        assert!(result.is_some(), "Fit should succeed");
        let result = result.unwrap();

        assert!(result.converged, "Fit should converge");
        assert!(
            (result.x - true_cx).abs() < 0.1,
            "X should be accurate: {} vs {}",
            result.x,
            true_cx
        );
        assert!(
            (result.y - true_cy).abs() < 0.1,
            "Y should be accurate: {} vs {}",
            result.y,
            true_cy
        );
        // Beta fitting is less accurate
        assert!(
            (result.beta - true_beta).abs() < 0.5,
            "Beta should be reasonably accurate: {} vs {}",
            result.beta,
            true_beta
        );
    }

    #[test]
    fn test_alpha_beta_fwhm_conversion() {
        let alpha = 2.0;
        let beta = 2.5;
        let fwhm = alpha_beta_to_fwhm(alpha, beta);
        let alpha_back = fwhm_beta_to_alpha(fwhm, beta);

        assert!(
            (alpha_back - alpha).abs() < 1e-6,
            "Round-trip should preserve alpha"
        );

        // For beta=2.5, FWHM = 2 * 2 * sqrt(2^0.4 - 1) ≈ 2.26
        assert!(
            (fwhm - 2.26).abs() < 0.1,
            "FWHM should be ~2.26 for alpha=2, beta=2.5: {}",
            fwhm
        );
    }

    #[test]
    fn test_moffat_fit_edge_position() {
        let width = 21;
        let height = 21;
        let pixels = vec![0.1f32; width * height];

        let config = MoffatFitConfig::default();
        let result = fit_moffat_2d(&pixels, width, height, 2.0, 10.0, 8, 0.1, &config);

        assert!(result.is_none(), "Fit should fail for edge position");
    }

    #[test]
    fn test_solve_5x5_simple() {
        let a = [
            [1.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 1.0],
        ];
        let b = [1.0, 2.0, 3.0, 4.0, 5.0];

        let x = solve_5x5(&a, &b);
        assert!(x.is_some());
        let x = x.unwrap();

        for i in 0..5 {
            assert!(
                (x[i] - b[i]).abs() < 1e-6,
                "Solution should match RHS for identity"
            );
        }
    }
}
