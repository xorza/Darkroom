//! Tests for 2D Gaussian fitting.

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

#[allow(clippy::too_many_arguments)]
fn make_gaussian_stamp_asymmetric(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    amplitude: f32,
    sigma_x: f32,
    sigma_y: f32,
    background: f32,
) -> Vec<f32> {
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let value = amplitude
                * (-0.5 * (dx * dx / (sigma_x * sigma_x) + dy * dy / (sigma_y * sigma_y))).exp();
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
    // Note: converged flag may be false if initial guess was already close
    // What matters is the accuracy of the result
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

    let pixels = make_gaussian_stamp_asymmetric(width, height, cx, cy, amp, sigma_x, sigma_y, bg);
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Note: converged flag may be false if initial guess was already close
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

// ============================================================================
// Additional tests for edge cases and noise
// ============================================================================

#[test]
fn test_gaussian_fit_high_snr() {
    // High SNR case - should achieve very accurate fit
    let width = 21;
    let height = 21;
    let true_cx = 10.25;
    let true_cy = 10.35;
    let true_amp = 100.0;
    let true_sigma = 2.0;
    let true_bg = 1.0;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!((result.x - true_cx).abs() < 0.02);
    assert!((result.y - true_cy).abs() < 0.02);
    assert!((result.amplitude - true_amp).abs() < 1.0);
}

#[test]
fn test_gaussian_fit_low_amplitude() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 0.05; // Very low amplitude
    let true_sigma = 2.5;
    let true_bg = 0.1;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    // Should still find something (amplitude is constrained to min 0.01)
    assert!(result.is_some());
}

#[test]
fn test_gaussian_fit_large_sigma() {
    let width = 31;
    let height = 31;
    let true_cx = 15.0;
    let true_cy = 15.0;
    let true_amp = 1.0;
    let true_sigma = 5.0; // Large sigma
    let true_bg = 0.1;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 15.0, 15.0, 12, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!((result.x - true_cx).abs() < 0.2);
    assert!((result.y - true_cy).abs() < 0.2);
    assert!((result.sigma_x - true_sigma).abs() < 0.5);
}

#[test]
fn test_gaussian_fit_small_sigma() {
    let width = 15;
    let height = 15;
    let true_cx = 7.0;
    let true_cy = 7.0;
    let true_amp = 1.0;
    let true_sigma = 1.0; // Small sigma (close to Nyquist)
    let true_bg = 0.1;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 7.0, 7.0, 5, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Smaller sigma means less pixels with signal, so accuracy may be lower
    assert!((result.x - true_cx).abs() < 0.15);
    assert!((result.y - true_cy).abs() < 0.15);
}

#[test]
fn test_gaussian_fit_with_noise() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;

    let mut pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );

    // Add deterministic "noise" pattern
    for (i, p) in pixels.iter_mut().enumerate() {
        let noise = 0.02 * ((i % 7) as f32 - 3.0) / 3.0;
        *p += noise;
    }

    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // With noise, accuracy is slightly reduced but should still be reasonable
    assert!((result.x - true_cx).abs() < 0.2);
    assert!((result.y - true_cy).abs() < 0.2);
}

#[test]
fn test_gaussian_fit_weighted() {
    let width = 21;
    let height = 21;
    let true_cx = 10.3;
    let true_cy = 10.7;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;
    let noise = 0.05;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d_weighted(
        &pixels_buf,
        10.0,
        11.0,
        8,
        true_bg,
        noise,
        Some(1.0), // gain
        Some(5.0), // read noise
        &config,
    );

    assert!(result.is_some());
    let result = result.unwrap();
    assert!((result.x - true_cx).abs() < 0.1);
    assert!((result.y - true_cy).abs() < 0.1);
}

#[test]
fn test_gaussian_fit_stamp_too_small() {
    let width = 5;
    let height = 5;
    let pixels = vec![0.5f32; width * height];
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    // Center at 2,2 with radius 3 would go outside the 5x5 image
    // extract_stamp returns None when stamp doesn't fit
    let result = fit_gaussian_2d(&pixels_buf, 2.0, 2.0, 3, 0.5, &config);
    assert!(result.is_none());
}

#[test]
fn test_gaussian_fit_zero_background() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.0;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!((result.x - true_cx).abs() < 0.1);
    assert!((result.y - true_cy).abs() < 0.1);
    assert!(result.background.abs() < 0.05);
}

#[test]
fn test_gaussian_fit_high_background() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 10.0; // High background

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!((result.x - true_cx).abs() < 0.1);
    assert!((result.y - true_cy).abs() < 0.1);
}

#[test]
fn test_gaussian_model_evaluate() {
    let model = Gaussian2D { stamp_radius: 8.0 };
    let params = [5.0f32, 5.0, 1.0, 2.0, 2.0, 0.1];

    // At center: f = amp * 1 + bg = 1.1
    let val_center = model.evaluate(5.0, 5.0, &params);
    assert!((val_center - 1.1).abs() < 1e-6);

    // Far from center: f approaches bg
    let val_far = model.evaluate(100.0, 100.0, &params);
    assert!((val_far - 0.1).abs() < 1e-6);
}

#[test]
fn test_gaussian_jacobian_at_center() {
    let model = Gaussian2D { stamp_radius: 8.0 };
    let params = [5.0f32, 5.0, 1.0, 2.0, 2.0, 0.1];

    let jac = model.jacobian_row(5.0, 5.0, &params);

    // At center (r=0):
    // df/dx0 = 0 (no gradient at center)
    // df/dy0 = 0
    // df/damp = exp(0) = 1
    // df/dsigma_x = 0 (no gradient)
    // df/dsigma_y = 0
    // df/dbg = 1
    assert!(jac[0].abs() < 1e-6);
    assert!(jac[1].abs() < 1e-6);
    assert!((jac[2] - 1.0).abs() < 1e-6);
    assert!(jac[3].abs() < 1e-6);
    assert!(jac[4].abs() < 1e-6);
    assert!((jac[5] - 1.0).abs() < 1e-6);
}

#[test]
fn test_gaussian_constrain() {
    let model = Gaussian2D { stamp_radius: 8.0 };

    // Test amplitude constraint
    let mut params = [5.0f32, 5.0, -1.0, 2.0, 2.0, 0.1];
    model.constrain(&mut params);
    assert!(params[2] >= 0.01);

    // Test sigma constraints
    let mut params = [5.0f32, 5.0, 1.0, 0.1, 0.1, 0.1]; // Too small
    model.constrain(&mut params);
    assert!(params[3] >= 0.5);
    assert!(params[4] >= 0.5);

    let mut params = [5.0f32, 5.0, 1.0, 100.0, 100.0, 0.1]; // Too large
    model.constrain(&mut params);
    assert!(params[3] <= 8.0);
    assert!(params[4] <= 8.0);
}

#[test]
fn test_fwhm_accuracy() {
    // Test that FWHM is correctly computed from sigma
    let sigma = 2.0;
    let fwhm = sigma_to_fwhm(sigma);

    // FWHM = 2 * sqrt(2 * ln(2)) * sigma ≈ 2.355 * sigma
    let expected = 2.0 * (2.0 * 2.0f32.ln()).sqrt() * sigma;
    assert!((fwhm - expected).abs() < 1e-5);
}

#[test]
fn test_gaussian_fit_multiple_positions() {
    // Test fitting at various subpixel positions
    let width = 21;
    let height = 21;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;

    for &(offset_x, offset_y) in &[
        (0.0, 0.0),
        (0.25, 0.25),
        (0.5, 0.5),
        (0.75, 0.75),
        (-0.3, 0.4),
    ] {
        let true_cx = 10.0 + offset_x;
        let true_cy = 10.0 + offset_y;

        let pixels = make_gaussian_stamp(
            width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
        );
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

        assert!(
            result.is_some(),
            "Failed for offset ({}, {})",
            offset_x,
            offset_y
        );
        let result = result.unwrap();
        assert!(
            (result.x - true_cx).abs() < 0.1,
            "X error {} for offset ({}, {})",
            (result.x - true_cx).abs(),
            offset_x,
            offset_y
        );
        assert!(
            (result.y - true_cy).abs() < 0.1,
            "Y error {} for offset ({}, {})",
            (result.y - true_cy).abs(),
            offset_x,
            offset_y
        );
    }
}
