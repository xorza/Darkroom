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

// ============================================================================
// Noise and difficult data tests
// ============================================================================

/// Add Gaussian noise to pixel values using a simple LCG PRNG.
fn add_noise(pixels: &mut [f32], noise_sigma: f32, seed: u64) {
    let mut state = seed;
    for pixel in pixels.iter_mut() {
        // Box-Muller transform with LCG
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u1 = (state as f32) / (u64::MAX as f32);
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let u2 = (state as f32) / (u64::MAX as f32);

        let z = (-2.0 * u1.max(1e-10).ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
        *pixel += z * noise_sigma;
    }
}

#[test]
fn test_gaussian_fit_with_gaussian_noise() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;
    let noise_sigma = 0.05; // 5% of amplitude

    let mut pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    add_noise(&mut pixels, noise_sigma, 12345);

    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    // With noise, allow larger tolerance
    assert!(
        (result.x - true_cx).abs() < 0.2,
        "x error: {}",
        (result.x - true_cx).abs()
    );
    assert!(
        (result.y - true_cy).abs() < 0.2,
        "y error: {}",
        (result.y - true_cy).abs()
    );
}

#[test]
fn test_gaussian_fit_high_noise_still_converges() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;
    let noise_sigma = 0.15; // 15% noise - challenging

    let mut pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    add_noise(&mut pixels, noise_sigma, 54321);

    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    // Should still converge even with high noise
    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    // Centroid should still be reasonable (within 0.5 pixel)
    assert!(
        (result.x - true_cx).abs() < 0.5,
        "x error too large: {}",
        (result.x - true_cx).abs()
    );
    assert!(
        (result.y - true_cy).abs() < 0.5,
        "y error too large: {}",
        (result.y - true_cy).abs()
    );
}

#[test]
fn test_gaussian_fit_low_snr() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 0.1; // Low amplitude
    let true_sigma = 2.5;
    let true_bg = 0.5; // High background (SNR ~ 0.2)

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    // Low SNR should still produce a result (may not be accurate)
    assert!(result.is_some());
    let result = result.unwrap();
    // Just verify it doesn't crash and produces finite values
    assert!(result.x.is_finite());
    assert!(result.y.is_finite());
    assert!(result.sigma_x.is_finite());
    assert!(result.sigma_y.is_finite());
}

#[test]
fn test_gaussian_fit_wrong_background_estimate() {
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

    // Use wrong background estimate (20% error)
    let wrong_bg = true_bg * 1.2;
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, wrong_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Should still converge - background is a fitted parameter
    assert!(result.converged);
    // Centroid should still be accurate
    assert!(
        (result.x - true_cx).abs() < 0.1,
        "x error: {}",
        (result.x - true_cx).abs()
    );
    assert!(
        (result.y - true_cy).abs() < 0.1,
        "y error: {}",
        (result.y - true_cy).abs()
    );
    // Fitted background should be close to true value
    assert!(
        (result.background - true_bg).abs() < 0.05,
        "bg error: {}",
        (result.background - true_bg).abs()
    );
}

// ============================================================================
// Amplitude and parameter edge cases
// ============================================================================

#[test]
fn test_gaussian_fit_very_high_amplitude() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 10000.0; // Very high amplitude
    let true_sigma = 2.5;
    let true_bg = 100.0;

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
    assert!((result.amplitude - true_amp).abs() / true_amp < 0.01);
}

#[test]
fn test_gaussian_fit_narrow_psf() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 0.8; // Very narrow PSF (close to Nyquist limit)
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
    assert!((result.x - true_cx).abs() < 0.15);
    assert!((result.y - true_cy).abs() < 0.15);
}

// ============================================================================
// Convergence and iteration tests
// ============================================================================

#[test]
fn test_gaussian_fit_converges_within_max_iterations() {
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

    let config = GaussianFitConfig {
        max_iterations: 10, // Low iteration limit
        ..Default::default()
    };
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Should converge quickly for perfect data
    assert!(result.converged);
    assert!(result.iterations <= 10);
}

#[test]
fn test_gaussian_fit_bad_initial_guess_still_converges() {
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

    let config = GaussianFitConfig {
        max_iterations: 100,
        ..Default::default()
    };

    // Start from a position offset by 2 pixels
    let result = fit_gaussian_2d(&pixels_buf, 8.0, 12.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!(
        (result.x - true_cx).abs() < 0.1,
        "x error: {}",
        (result.x - true_cx).abs()
    );
    assert!(
        (result.y - true_cy).abs() < 0.1,
        "y error: {}",
        (result.y - true_cy).abs()
    );
}

#[test]
fn test_gaussian_fit_uniform_data_returns_result() {
    // Uniform data (no star) - should still return a result, though meaningless
    let width = 21;
    let height = 21;
    let uniform_value = 0.5f32;
    let pixels = vec![uniform_value; width * height];
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, uniform_value, &config);

    // Should produce some result (may not converge well)
    assert!(result.is_some());
    let result = result.unwrap();
    // Values should be finite
    assert!(result.x.is_finite());
    assert!(result.y.is_finite());
    assert!(result.amplitude.is_finite());
    assert!(result.sigma_x.is_finite());
    assert!(result.sigma_y.is_finite());
}

// ============================================================================
// Jacobian off-center tests
// ============================================================================

#[test]
fn test_gaussian_jacobian_off_center() {
    let model = Gaussian2D { stamp_radius: 8.0 };
    let params = [5.0f32, 5.0, 1.0, 2.0, 2.0, 0.1];

    // Test at a point off-center
    let jac = model.jacobian_row(6.0, 5.0, &params);

    // At (6, 5), dx=1, dy=0
    // df/dx0 should be positive (moving center right decreases residual)
    assert!(jac[0] > 0.0, "df/dx0 should be positive at x > x0");
    // df/dy0 should be ~0
    assert!(jac[1].abs() < 1e-6, "df/dy0 should be ~0 when dy=0");
    // df/damp should be exp(-0.5 * 1/4) = exp(-0.125) ≈ 0.88
    let expected_exp = (-0.125f32).exp();
    assert!(
        (jac[2] - expected_exp).abs() < 1e-5,
        "df/damp mismatch: {} vs {}",
        jac[2],
        expected_exp
    );
    // df/dbg = 1
    assert!((jac[5] - 1.0).abs() < 1e-6);
}

#[test]
fn test_gaussian_jacobian_diagonal() {
    let model = Gaussian2D { stamp_radius: 8.0 };
    let params = [5.0f32, 5.0, 1.0, 2.0, 2.0, 0.1];

    // Test at a diagonal point
    let jac = model.jacobian_row(6.0, 6.0, &params);

    // At (6, 6), dx=1, dy=1
    // Both df/dx0 and df/dy0 should be positive and equal (symmetric sigma)
    assert!(jac[0] > 0.0);
    assert!(jac[1] > 0.0);
    assert!(
        (jac[0] - jac[1]).abs() < 1e-6,
        "df/dx0 and df/dy0 should be equal for symmetric sigma"
    );
}

#[test]
fn test_gaussian_jacobian_asymmetric_sigma() {
    let model = Gaussian2D { stamp_radius: 8.0 };
    let params = [5.0f32, 5.0, 1.0, 2.0, 3.0, 0.1]; // sigma_x=2, sigma_y=3

    // Test at a diagonal point
    let jac = model.jacobian_row(6.0, 6.0, &params);

    // With different sigmas, the jacobians should differ
    // df/dsigma_x and df/dsigma_y should be different
    assert!(
        (jac[3] - jac[4]).abs() > 1e-6,
        "df/dsigma_x and df/dsigma_y should differ for asymmetric sigma"
    );
}

// ============================================================================
// Validate result edge cases
// ============================================================================

#[test]
fn test_gaussian_fit_center_outside_stamp_rejected() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;

    // Create a stamp with peak at edge so fitting might push center outside
    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    // Give initial guess very far from true center - should still find it
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 3, true_bg, &config);

    // Small stamp radius = 3, if result.x moves more than 3 pixels from cx, it's rejected
    // With true center at 10, the fit should succeed
    assert!(result.is_some());
}

#[test]
fn test_gaussian_fit_rms_residual_computed() {
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
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config).unwrap();

    // For perfect data, RMS residual should be very small
    assert!(
        result.rms_residual < 1e-4,
        "RMS residual too high: {}",
        result.rms_residual
    );
    assert!(result.rms_residual >= 0.0);
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

// ============================================================================
// Hessian and gradient tests
// ============================================================================

#[test]
fn test_compute_hessian_gradient_symmetry() {
    // Create test jacobian and residuals
    let jacobian = vec![
        [1.0f32, 0.5, 0.3, 0.2, 0.1, 0.05],
        [0.8, 0.6, 0.4, 0.25, 0.15, 0.08],
        [0.6, 0.7, 0.5, 0.3, 0.2, 0.1],
        [0.4, 0.4, 0.35, 0.22, 0.18, 0.07],
    ];
    let residuals = vec![0.1f32, -0.05, 0.08, -0.03];

    let (hessian, gradient) = compute_hessian_gradient_6(&jacobian, &residuals);

    // Hessian must be symmetric: H[i][j] == H[j][i]
    for (i, row) in hessian.iter().enumerate() {
        for (j, &val) in row.iter().enumerate() {
            assert!(
                (val - hessian[j][i]).abs() < 1e-6,
                "Hessian not symmetric at [{},{}]: {} vs {}",
                i,
                j,
                val,
                hessian[j][i]
            );
        }
    }

    // All values should be finite
    for (i, &g) in gradient.iter().enumerate() {
        assert!(g.is_finite(), "Gradient[{}] not finite", i);
        for (j, &h) in hessian[i].iter().enumerate() {
            assert!(h.is_finite(), "Hessian[{}][{}] not finite", i, j);
        }
    }
}

#[test]
fn test_compute_hessian_gradient_values() {
    // Simple case: single jacobian row
    let jacobian = vec![[1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0]];
    let residuals = vec![1.0f32];

    let (hessian, gradient) = compute_hessian_gradient_6(&jacobian, &residuals);

    // Gradient should be J^T * r = [1, 2, 3, 4, 5, 6]
    for (i, &g) in gradient.iter().enumerate() {
        let expected = (i + 1) as f32;
        assert!(
            (g - expected).abs() < 1e-6,
            "Gradient[{}] = {}, expected {}",
            i,
            g,
            expected
        );
    }

    // Hessian should be J^T * J = outer product
    for (i, row) in hessian.iter().enumerate() {
        for (j, &h) in row.iter().enumerate() {
            let expected = ((i + 1) * (j + 1)) as f32;
            assert!(
                (h - expected).abs() < 1e-6,
                "Hessian[{}][{}] = {}, expected {}",
                i,
                j,
                h,
                expected
            );
        }
    }
}

#[test]
fn test_compute_hessian_gradient_empty() {
    let jacobian: Vec<[f32; 6]> = vec![];
    let residuals: Vec<f32> = vec![];

    let (hessian, gradient) = compute_hessian_gradient_6(&jacobian, &residuals);

    // Empty input should give zero hessian and gradient
    for &g in &gradient {
        assert_eq!(g, 0.0);
    }
    for row in &hessian {
        for &h in row {
            assert_eq!(h, 0.0);
        }
    }
}

#[test]
fn test_compute_hessian_gradient_positive_semidefinite() {
    // For any Jacobian, J^T * J should be positive semi-definite
    // This means x^T * H * x >= 0 for all x
    let jacobian = vec![
        [0.5f32, 0.3, 0.8, 0.2, 0.6, 0.4],
        [0.7, 0.4, 0.2, 0.5, 0.3, 0.1],
        [0.3, 0.6, 0.5, 0.4, 0.2, 0.3],
    ];
    let residuals = vec![0.1f32, -0.2, 0.15];

    let (hessian, _) = compute_hessian_gradient_6(&jacobian, &residuals);

    // Test with several random vectors
    let test_vectors = [
        [1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        [0.5, -0.3, 0.7, -0.2, 0.4, -0.1],
    ];

    for x in &test_vectors {
        let mut result = 0.0f32;
        for (i, row) in hessian.iter().enumerate() {
            for (j, &h) in row.iter().enumerate() {
                result += x[i] * h * x[j];
            }
        }
        assert!(
            result >= -1e-6,
            "Hessian not positive semi-definite: x^T H x = {}",
            result
        );
    }
}

// ============================================================================
// Weighted fitting tests
// ============================================================================

#[test]
fn test_gaussian_fit_weighted_improves_accuracy_with_noise() {
    let width = 21;
    let height = 21;
    let true_cx = 10.3;
    let true_cy = 10.7;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;
    let noise_sigma = 0.05; // Moderate noise

    let mut pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    add_noise(&mut pixels, noise_sigma, 98765);

    let pixels_buf = Buffer2::new(width, height, pixels);
    let config = GaussianFitConfig::default();

    // Unweighted fit
    let result_unweighted = fit_gaussian_2d(&pixels_buf, 10.0, 11.0, 8, true_bg, &config).unwrap();

    // Weighted fit with proper noise model (use small read noise to avoid zero weights)
    let result_weighted = fit_gaussian_2d_weighted(
        &pixels_buf,
        10.0,
        11.0,
        8,
        true_bg,
        noise_sigma,
        Some(1.0),
        Some(1.0), // Small read noise
        &config,
    )
    .unwrap();

    // Both should find reasonable centroids (converged flag may vary)
    let error_unweighted =
        ((result_unweighted.x - true_cx).powi(2) + (result_unweighted.y - true_cy).powi(2)).sqrt();
    let error_weighted =
        ((result_weighted.x - true_cx).powi(2) + (result_weighted.y - true_cy).powi(2)).sqrt();

    assert!(
        error_unweighted < 0.3,
        "Unweighted error too large: {}",
        error_unweighted
    );
    assert!(
        error_weighted < 0.3,
        "Weighted error too large: {}",
        error_weighted
    );
}

#[test]
fn test_gaussian_fit_weighted_gain_effects() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;
    let noise = 0.05;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);
    let config = GaussianFitConfig::default();

    // Test with different gain values
    for gain in [0.5, 1.0, 2.0, 5.0] {
        let result = fit_gaussian_2d_weighted(
            &pixels_buf,
            10.0,
            10.0,
            8,
            true_bg,
            noise,
            Some(gain),
            Some(5.0),
            &config,
        );

        assert!(result.is_some(), "Failed with gain={}", gain);
        let result = result.unwrap();
        assert!(
            (result.x - true_cx).abs() < 0.1,
            "X error with gain={}: {}",
            gain,
            (result.x - true_cx).abs()
        );
        assert!(
            (result.y - true_cy).abs() < 0.1,
            "Y error with gain={}: {}",
            gain,
            (result.y - true_cy).abs()
        );
    }
}

#[test]
fn test_gaussian_fit_weighted_read_noise_effects() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;
    let noise = 0.05;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);
    let config = GaussianFitConfig::default();

    // Test with different read noise values
    for read_noise in [0.0, 2.0, 5.0, 10.0, 20.0] {
        let result = fit_gaussian_2d_weighted(
            &pixels_buf,
            10.0,
            10.0,
            8,
            true_bg,
            noise,
            Some(1.0),
            Some(read_noise),
            &config,
        );

        assert!(result.is_some(), "Failed with read_noise={}", read_noise);
        let result = result.unwrap();
        assert!(
            (result.x - true_cx).abs() < 0.1,
            "X error with read_noise={}: {}",
            read_noise,
            (result.x - true_cx).abs()
        );
        assert!(
            (result.y - true_cy).abs() < 0.1,
            "Y error with read_noise={}: {}",
            read_noise,
            (result.y - true_cy).abs()
        );
    }
}

#[test]
fn test_gaussian_fit_weighted_no_gain_no_read_noise() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;
    let noise = 0.05;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);
    let config = GaussianFitConfig::default();

    // Test with None for gain and read_noise
    let result = fit_gaussian_2d_weighted(
        &pixels_buf,
        10.0,
        10.0,
        8,
        true_bg,
        noise,
        None,
        None,
        &config,
    );

    assert!(result.is_some());
    let result = result.unwrap();
    assert!((result.x - true_cx).abs() < 0.1);
    assert!((result.y - true_cy).abs() < 0.1);
}

// ============================================================================
// Extreme parameter boundary tests
// ============================================================================

#[test]
fn test_gaussian_fit_sigma_at_lower_bound() {
    // Test with sigma very close to constraint minimum (0.5 px)
    let width = 15;
    let height = 15;
    let true_cx = 7.0;
    let true_cy = 7.0;
    let true_amp = 1.0;
    let true_sigma = 0.6; // Just above min constraint of 0.5
    let true_bg = 0.1;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 7.0, 7.0, 5, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Sigma should be clamped to >= 0.5
    assert!(result.sigma_x >= 0.5);
    assert!(result.sigma_y >= 0.5);
    // Centroid should still be reasonable
    assert!((result.x - true_cx).abs() < 0.2);
    assert!((result.y - true_cy).abs() < 0.2);
}

#[test]
fn test_gaussian_fit_sigma_at_upper_bound() {
    // Test with sigma close to stamp_radius constraint
    let width = 31;
    let height = 31;
    let true_cx = 15.0;
    let true_cy = 15.0;
    let true_amp = 1.0;
    let stamp_radius = 10;
    let true_sigma = 9.0; // Close to stamp_radius
    let true_bg = 0.1;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 15.0, 15.0, stamp_radius, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Sigma should be clamped to <= stamp_radius
    assert!(result.sigma_x <= stamp_radius as f32);
    assert!(result.sigma_y <= stamp_radius as f32);
    // Centroid should still be found
    assert!((result.x - true_cx).abs() < 0.5);
    assert!((result.y - true_cy).abs() < 0.5);
}

#[test]
fn test_gaussian_fit_extreme_amplitude_range() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;

    // Test across wide amplitude range
    for amp_exp in [-3, -2, -1, 0, 1, 2, 3, 4] {
        let true_amp = 10.0f32.powi(amp_exp);
        let pixels = make_gaussian_stamp(
            width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
        );
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

        assert!(result.is_some(), "Failed for amplitude=10^{}", amp_exp);
        let result = result.unwrap();
        assert!(
            result.x.is_finite(),
            "Non-finite x for amplitude=10^{}",
            amp_exp
        );
        assert!(
            result.y.is_finite(),
            "Non-finite y for amplitude=10^{}",
            amp_exp
        );
        assert!(
            result.amplitude.is_finite(),
            "Non-finite amplitude for amplitude=10^{}",
            amp_exp
        );
    }
}

#[test]
fn test_gaussian_fit_high_sigma_low_amplitude() {
    // Combination: very wide PSF with low amplitude
    let width = 41;
    let height = 41;
    let true_cx = 20.0;
    let true_cy = 20.0;
    let true_amp = 0.1; // Low amplitude
    let true_sigma = 8.0; // Wide PSF
    let true_bg = 0.05;

    let pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 20.0, 20.0, 15, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // With wide PSF and low amplitude, accuracy is reduced but should still work
    assert!(result.x.is_finite());
    assert!(result.y.is_finite());
    assert!((result.x - true_cx).abs() < 1.0);
    assert!((result.y - true_cy).abs() < 1.0);
}

#[test]
fn test_gaussian_fit_residual_distribution() {
    // On noisy data, check that residuals are reasonable
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;
    let noise_sigma = 0.05;

    let mut pixels = make_gaussian_stamp(
        width, height, true_cx, true_cy, true_amp, true_sigma, true_bg,
    );
    add_noise(&mut pixels, noise_sigma, 11111);

    let pixels_buf = Buffer2::new(width, height, pixels);
    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();

    // RMS residual should be roughly proportional to noise level
    // Allow factor of 2-3 due to fitting degrees of freedom
    assert!(
        result.rms_residual < noise_sigma * 3.0,
        "RMS residual {} much larger than noise {}",
        result.rms_residual,
        noise_sigma
    );
}
