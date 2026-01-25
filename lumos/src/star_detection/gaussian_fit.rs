//! 2D Gaussian fitting for high-precision centroid computation.
//!
//! Implements Levenberg-Marquardt optimization to fit a 2D Gaussian model:
//! f(x,y) = A × exp(-((x-x₀)²/2σ_x² + (y-y₀)²/2σ_y²)) + B
//!
//! This achieves ~0.01 pixel centroid accuracy compared to ~0.05 for weighted centroid.

// These are public API functions exported for external use
#![allow(dead_code)]

/// Configuration for Gaussian fitting.
#[derive(Debug, Clone)]
pub struct GaussianFitConfig {
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
}

impl Default for GaussianFitConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            convergence_threshold: 1e-6,
            initial_lambda: 0.001,
            lambda_up: 10.0,
            lambda_down: 0.1,
        }
    }
}

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

/// Fit a 2D Gaussian to a star stamp.
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
/// `Some(GaussianFitResult)` if fit succeeds, `None` if fitting fails.
#[allow(clippy::too_many_arguments)]
pub fn fit_gaussian_2d(
    pixels: &[f32],
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
    background: f32,
    config: &GaussianFitConfig,
) -> Option<GaussianFitResult> {
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
    if n < 7 {
        // Need at least 7 points to fit 6 parameters
        return None;
    }

    // Initial parameter estimates
    // [x0, y0, amplitude, sigma_x, sigma_y, background]
    let peak_value = data_z.iter().fold(f32::MIN, |a, &b| a.max(b));
    let mut params = [
        cx,
        cy,
        (peak_value - background).max(0.01),
        2.0, // Initial sigma estimate
        2.0,
        background,
    ];

    let mut lambda = config.initial_lambda;
    let mut prev_chi2 = compute_chi2(&data_x, &data_y, &data_z, &params);
    let mut converged = false;
    let mut iterations = 0;

    for iter in 0..config.max_iterations {
        iterations = iter + 1;

        // Compute Jacobian and Hessian approximation
        let (jacobian, residuals) = compute_jacobian(&data_x, &data_y, &data_z, &params);
        let hessian = compute_hessian(&jacobian);
        let gradient = compute_gradient(&jacobian, &residuals);

        // Levenberg-Marquardt update
        let mut damped_hessian = hessian;
        for (i, row) in damped_hessian.iter_mut().enumerate() {
            row[i] *= 1.0 + lambda;
        }

        // Solve linear system: damped_hessian * delta = -gradient
        let delta = match solve_6x6(&damped_hessian, &gradient) {
            Some(d) => d,
            None => break, // Singular matrix
        };

        // Try update
        // For L-M, we solve H * delta = gradient where gradient = J^T * r
        // and r = (observed - model), so we want params + delta to minimize residuals
        let mut new_params = params;
        for (p, d) in new_params.iter_mut().zip(delta.iter()) {
            *p += d;
        }

        // Constrain parameters to reasonable values
        new_params[2] = new_params[2].max(0.01); // Amplitude > 0
        new_params[3] = new_params[3].clamp(0.5, stamp_radius as f32); // Sigma_x
        new_params[4] = new_params[4].clamp(0.5, stamp_radius as f32); // Sigma_y

        let new_chi2 = compute_chi2(&data_x, &data_y, &data_z, &new_params);

        if new_chi2 < prev_chi2 {
            // Accept step
            params = new_params;
            lambda *= config.lambda_down;
            prev_chi2 = new_chi2;

            // Check convergence
            let max_delta = delta.iter().map(|d| d.abs()).fold(0.0f32, f32::max);
            if max_delta < config.convergence_threshold {
                converged = true;
                break;
            }
        } else {
            // Reject step, increase damping
            lambda *= config.lambda_up;

            if lambda > 1e10 {
                break; // Too much damping, give up
            }
        }
    }

    // Validate result
    let [x0, y0, amplitude, sigma_x, sigma_y, bg] = params;

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

    // Compute RMS residual
    let rms = (prev_chi2 / n as f32).sqrt();

    Some(GaussianFitResult {
        x: x0,
        y: y0,
        amplitude,
        sigma_x,
        sigma_y,
        background: bg,
        rms_residual: rms,
        converged,
        iterations,
    })
}

/// Evaluate 2D Gaussian at a point.
#[inline]
fn gaussian_2d(x: f32, y: f32, params: &[f32; 6]) -> f32 {
    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let dx = x - x0;
    let dy = y - y0;
    let exponent = -0.5 * (dx * dx / (sigma_x * sigma_x) + dy * dy / (sigma_y * sigma_y));
    amp * exponent.exp() + bg
}

/// Compute chi-squared (sum of squared residuals).
fn compute_chi2(data_x: &[f32], data_y: &[f32], data_z: &[f32], params: &[f32; 6]) -> f32 {
    data_x
        .iter()
        .zip(data_y.iter())
        .zip(data_z.iter())
        .map(|((&x, &y), &z)| {
            let model = gaussian_2d(x, y, params);
            let residual = z - model;
            residual * residual
        })
        .sum()
}

/// Compute Jacobian matrix (partial derivatives of residuals w.r.t. parameters).
fn compute_jacobian(
    data_x: &[f32],
    data_y: &[f32],
    data_z: &[f32],
    params: &[f32; 6],
) -> (Vec<[f32; 6]>, Vec<f32>) {
    let [x0, y0, amp, sigma_x, sigma_y, _bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;

    let n = data_x.len();
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for i in 0..n {
        let x = data_x[i];
        let y = data_y[i];
        let z = data_z[i];

        let dx = x - x0;
        let dy = y - y0;
        let exponent = -0.5 * (dx * dx / sigma_x2 + dy * dy / sigma_y2);
        let exp_val = exponent.exp();
        let model = amp * exp_val + params[5];

        // Partial derivatives
        // df/dx0 = amp * exp * (dx / sigma_x²)
        let df_dx0 = amp * exp_val * dx / sigma_x2;
        // df/dy0 = amp * exp * (dy / sigma_y²)
        let df_dy0 = amp * exp_val * dy / sigma_y2;
        // df/damp = exp
        let df_damp = exp_val;
        // df/dsigma_x = amp * exp * dx² / sigma_x³
        let df_dsigma_x = amp * exp_val * dx * dx / (sigma_x2 * sigma_x);
        // df/dsigma_y = amp * exp * dy² / sigma_y³
        let df_dsigma_y = amp * exp_val * dy * dy / (sigma_y2 * sigma_y);
        // df/dbg = 1
        let df_dbg = 1.0;

        jacobian.push([df_dx0, df_dy0, df_damp, df_dsigma_x, df_dsigma_y, df_dbg]);
        residuals.push(z - model);
    }

    (jacobian, residuals)
}

/// Compute Hessian approximation (J^T * J).
fn compute_hessian(jacobian: &[[f32; 6]]) -> [[f32; 6]; 6] {
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

/// Compute gradient (J^T * residuals).
fn compute_gradient(jacobian: &[[f32; 6]], residuals: &[f32]) -> [f32; 6] {
    let mut gradient = [0.0f32; 6];

    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..6 {
            gradient[i] += row[i] * r;
        }
    }

    gradient
}

/// Solve 6x6 linear system using Gaussian elimination with partial pivoting.
#[allow(clippy::needless_range_loop)]
fn solve_6x6(a: &[[f32; 6]; 6], b: &[f32; 6]) -> Option<[f32; 6]> {
    // Copy to working arrays
    let mut matrix = *a;
    let mut rhs = *b;

    // Forward elimination with partial pivoting
    for col in 0..6 {
        // Find pivot
        let mut max_row = col;
        let mut max_val = matrix[col][col].abs();
        for row in (col + 1)..6 {
            if matrix[row][col].abs() > max_val {
                max_val = matrix[row][col].abs();
                max_row = row;
            }
        }

        if max_val < 1e-10 {
            return None; // Singular matrix
        }

        // Swap rows
        if max_row != col {
            matrix.swap(col, max_row);
            rhs.swap(col, max_row);
        }

        // Eliminate column
        for row in (col + 1)..6 {
            let factor = matrix[row][col] / matrix[col][col];
            let pivot_row = matrix[col];
            for (j, m) in matrix[row].iter_mut().enumerate().skip(col) {
                *m -= factor * pivot_row[j];
            }
            rhs[row] -= factor * rhs[col];
        }
    }

    // Back substitution
    let mut x = [0.0f32; 6];
    for i in (0..6).rev() {
        let mut sum = rhs[i];
        for (j, &xj) in x.iter().enumerate().skip(i + 1) {
            sum -= matrix[i][j] * xj;
        }
        x[i] = sum / matrix[i][i];
    }

    Some(x)
}

/// Convert sigma to FWHM.
#[inline]
pub fn sigma_to_fwhm(sigma: f32) -> f32 {
    sigma * 2.0 * (2.0 * (2.0f32).ln()).sqrt() // 2 * sqrt(2 * ln(2)) ≈ 2.355
}

/// Convert FWHM to sigma.
#[inline]
pub fn fwhm_to_sigma(fwhm: f32) -> f32 {
    fwhm / (2.0 * (2.0 * (2.0f32).ln()).sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gaussian_stamp(
        width: usize,
        height: usize,
        cx: f32,
        cy: f32,
        amplitude: f32,
        sigma: f32,
        background: f32,
    ) -> Vec<f32> {
        let mut pixels = vec![background; width * height];
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let value = amplitude * (-0.5 * (dx * dx + dy * dy) / (sigma * sigma)).exp();
                pixels[y * width + x] += value;
            }
        }
        pixels
    }

    #[test]
    fn test_gaussian_fit_centered() {
        let width = 21;
        let height = 21;
        let true_cx = 10.0;
        let true_cy = 10.0;
        let true_amp = 1.0;
        let true_sigma = 2.5;
        let true_bg = 0.1;

        let pixels = make_gaussian_stamp(
            width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
        );

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels, width, height, 10.0, 10.0, 8, true_bg, &config);

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
            (result.sigma_x - true_sigma).abs() < 0.2,
            "Sigma_x should be accurate"
        );
        assert!(
            (result.sigma_y - true_sigma).abs() < 0.2,
            "Sigma_y should be accurate"
        );
    }

    #[test]
    fn test_gaussian_fit_subpixel_offset() {
        let width = 21;
        let height = 21;
        let true_cx = 10.3; // Sub-pixel offset
        let true_cy = 10.7;
        let true_amp = 1.0;
        let true_sigma = 2.5;
        let true_bg = 0.1;

        let pixels = make_gaussian_stamp(
            width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
        );

        let config = GaussianFitConfig::default();
        // Start from integer position
        let result = fit_gaussian_2d(&pixels, width, height, 10.0, 11.0, 8, true_bg, &config);

        assert!(result.is_some(), "Fit should succeed");
        let result = result.unwrap();

        assert!(result.converged, "Fit should converge");
        // Gaussian fitting should achieve ~0.01 pixel accuracy
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
    fn test_gaussian_fit_asymmetric() {
        let width = 21;
        let height = 21;
        let cx = 10.0;
        let cy = 10.0;
        let amp = 1.0;
        let sigma_x = 2.0;
        let sigma_y = 3.0;
        let bg = 0.1;

        // Create asymmetric Gaussian
        let mut pixels = vec![bg; width * height];
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let value = amp
                    * (-0.5 * (dx * dx / (sigma_x * sigma_x) + dy * dy / (sigma_y * sigma_y)))
                        .exp();
                pixels[y * width + x] += value;
            }
        }

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels, width, height, 10.0, 10.0, 8, bg, &config);

        assert!(result.is_some(), "Fit should succeed");
        let result = result.unwrap();

        assert!(result.converged, "Fit should converge");
        assert!(
            (result.sigma_x - sigma_x).abs() < 0.3,
            "Sigma_x should be accurate: {} vs {}",
            result.sigma_x,
            sigma_x
        );
        assert!(
            (result.sigma_y - sigma_y).abs() < 0.3,
            "Sigma_y should be accurate: {} vs {}",
            result.sigma_y,
            sigma_y
        );
    }

    #[test]
    fn test_sigma_fwhm_conversion() {
        let sigma = 2.0;
        let fwhm = sigma_to_fwhm(sigma);
        let sigma_back = fwhm_to_sigma(fwhm);

        assert!(
            (sigma_back - sigma).abs() < 1e-6,
            "Round-trip should preserve sigma"
        );
        assert!((fwhm - 4.71).abs() < 0.01, "FWHM should be ~2.355 * sigma");
    }

    #[test]
    fn test_gaussian_fit_edge_position() {
        let width = 21;
        let height = 21;
        let pixels = vec![0.1f32; width * height];

        let config = GaussianFitConfig::default();
        // Position too close to edge
        let result = fit_gaussian_2d(&pixels, width, height, 2.0, 10.0, 8, 0.1, &config);

        assert!(result.is_none(), "Fit should fail for edge position");
    }

    #[test]
    fn test_solve_6x6_simple() {
        // Identity matrix
        let a = [
            [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        ];
        let b = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];

        let x = solve_6x6(&a, &b);
        assert!(x.is_some());
        let x = x.unwrap();

        for i in 0..6 {
            assert!(
                (x[i] - b[i]).abs() < 1e-6,
                "Solution should match RHS for identity"
            );
        }
    }
}
