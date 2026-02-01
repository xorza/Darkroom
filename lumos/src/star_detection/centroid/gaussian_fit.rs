//! 2D Gaussian fitting for high-precision centroid computation.
//!
//! Implements Levenberg-Marquardt optimization to fit a 2D Gaussian model:
//! f(x,y) = A × exp(-((x-x₀)²/2σ_x² + (y-y₀)²/2σ_y²)) + B
//!
//! This achieves ~0.01 pixel centroid accuracy compared to ~0.05 for weighted centroid.

#![allow(dead_code)]

use super::lm_optimizer::{LMConfig, LMModel, LMResult, optimize_6};
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
struct Gaussian2D {
    stamp_radius: f32,
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
        let exponent = -0.5 * (dx * dx / sigma_x2 + dy * dy / sigma_y2);
        let exp_val = exponent.exp();

        [
            amp * exp_val * dx / sigma_x2,                  // df/dx0
            amp * exp_val * dy / sigma_y2,                  // df/dy0
            exp_val,                                        // df/damp
            amp * exp_val * dx * dx / (sigma_x2 * sigma_x), // df/dsigma_x
            amp * exp_val * dy * dy / (sigma_y2 * sigma_y), // df/dsigma_y
            1.0,                                            // df/dbg
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
///
/// # Arguments
/// * `pixels` - Image pixel data
/// * `cx` - Initial X estimate (from weighted centroid)
/// * `cy` - Initial Y estimate (from weighted centroid)
/// * `stamp_radius` - Radius of stamp to fit
/// * `background` - Background estimate
/// * `config` - Fitting configuration
///
/// # Returns
/// `Some(GaussianFitResult)` if fit succeeds, `None` if fitting fails.
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

    let initial_params = [
        cx,
        cy,
        (peak_value - background).max(0.01),
        2.0,
        2.0,
        background,
    ];

    let model = Gaussian2D {
        stamp_radius: stamp_radius as f32,
    };
    let result = optimize_6(&model, &data_x, &data_y, &data_z, initial_params, config);

    validate_result(&result, cx, cy, stamp_radius, n)
}

/// Extracted stamp data: (x coords, y coords, values, peak value).
type StampData = (Vec<f32>, Vec<f32>, Vec<f32>, f32);

fn extract_stamp(
    pixels: &Buffer2<f32>,
    cx: f32,
    cy: f32,
    stamp_radius: usize,
) -> Option<StampData> {
    let width = pixels.width();
    let height = pixels.height();
    let icx = cx.round() as isize;
    let icy = cy.round() as isize;

    if icx < stamp_radius as isize
        || icy < stamp_radius as isize
        || icx >= (width - stamp_radius) as isize
        || icy >= (height - stamp_radius) as isize
    {
        return None;
    }

    let stamp_radius_i32 = stamp_radius as i32;
    let mut data_x = Vec::new();
    let mut data_y = Vec::new();
    let mut data_z = Vec::new();
    let mut peak_value = f32::MIN;

    for dy in -stamp_radius_i32..=stamp_radius_i32 {
        for dx in -stamp_radius_i32..=stamp_radius_i32 {
            let x = (icx + dx as isize) as usize;
            let y = (icy + dy as isize) as usize;
            let value = pixels[y * width + x];

            data_x.push(x as f32);
            data_y.push(y as f32);
            data_z.push(value);
            peak_value = peak_value.max(value);
        }
    }

    Some((data_x, data_y, data_z, peak_value))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{fwhm_to_sigma, sigma_to_fwhm};

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
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.converged);
        assert!((result.x - true_cx).abs() < 0.1);
        assert!((result.y - true_cy).abs() < 0.1);
        assert!((result.sigma_x - true_sigma).abs() < 0.2);
        assert!((result.sigma_y - true_sigma).abs() < 0.2);
    }

    #[test]
    fn test_gaussian_fit_subpixel_offset() {
        let width = 21;
        let height = 21;
        let true_cx = 10.3;
        let true_cy = 10.7;
        let true_amp = 1.0;
        let true_sigma = 2.5;
        let true_bg = 0.1;

        let pixels = make_gaussian_stamp(
            width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
        );
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels_buf, 10.0, 11.0, 8, true_bg, &config);

        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.converged);
        assert!((result.x - true_cx).abs() < 0.05);
        assert!((result.y - true_cy).abs() < 0.05);
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
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, bg, &config);

        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.converged);
        assert!((result.sigma_x - sigma_x).abs() < 0.3);
        assert!((result.sigma_y - sigma_y).abs() < 0.3);
    }

    #[test]
    fn test_sigma_fwhm_conversion() {
        let sigma = 2.0;
        let fwhm = sigma_to_fwhm(sigma);
        let sigma_back = fwhm_to_sigma(fwhm);
        assert!((sigma_back - sigma).abs() < 1e-6);
        assert!((fwhm - 4.71).abs() < 0.01);
    }

    #[test]
    fn test_gaussian_fit_edge_position() {
        let width = 21;
        let height = 21;
        let pixels = vec![0.1f32; width * height];
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels_buf, 2.0, 10.0, 8, 0.1, &config);
        assert!(result.is_none());
    }
}
