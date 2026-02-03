//! Tests for Moffat profile fitting.

use super::*;
use crate::star_detection::centroid::lm_optimizer::LMConfig;

#[allow(clippy::too_many_arguments)]
pub(crate) fn make_moffat_stamp(
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
    assert!((result.pos.x as f32 - true_cx).abs() < 0.1);
    assert!((result.pos.y as f32 - true_cy).abs() < 0.1);
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
    assert!((result.pos.x as f32 - true_cx).abs() < 0.05);
    assert!((result.pos.y as f32 - true_cy).abs() < 0.05);
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
    assert!((result.pos.x as f32 - true_cx).abs() < 0.1);
    assert!((result.pos.y as f32 - true_cy).abs() < 0.1);
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
fn test_moffat_fit_with_gaussian_noise() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 0.1;
    let noise_sigma = 0.05; // 5% of amplitude

    let mut pixels = make_moffat_stamp(
        width, height, true_cx, true_cy, true_amp, true_alpha, true_beta, true_bg,
    );
    add_noise(&mut pixels, noise_sigma, 12345);

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
    // With noise, allow larger tolerance
    assert!(
        (result.pos.x as f32 - true_cx).abs() < 0.2,
        "x error: {}",
        (result.pos.x as f32 - true_cx).abs()
    );
    assert!(
        (result.pos.y as f32 - true_cy).abs() < 0.2,
        "y error: {}",
        (result.pos.y as f32 - true_cy).abs()
    );
}

#[test]
fn test_moffat_fit_high_noise_still_converges() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 0.1;
    let noise_sigma = 0.15; // 15% noise - challenging

    let mut pixels = make_moffat_stamp(
        width, height, true_cx, true_cy, true_amp, true_alpha, true_beta, true_bg,
    );
    add_noise(&mut pixels, noise_sigma, 54321);

    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: true_beta,
        ..Default::default()
    };
    let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    // Should still converge even with high noise
    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    // Centroid should still be reasonable (within 0.5 pixel)
    assert!(
        (result.pos.x as f32 - true_cx).abs() < 0.5,
        "x error too large: {}",
        (result.pos.x as f32 - true_cx).abs()
    );
    assert!(
        (result.pos.y as f32 - true_cy).abs() < 0.5,
        "y error too large: {}",
        (result.pos.y as f32 - true_cy).abs()
    );
}

#[test]
fn test_moffat_fit_low_snr() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 0.1; // Low amplitude
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 0.5; // High background (SNR ~ 0.2)

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

    // Low SNR should still produce a result (may not be accurate)
    assert!(result.is_some());
    let result = result.unwrap();
    // Just verify it doesn't crash and produces finite values
    assert!((result.pos.x as f32).is_finite());
    assert!((result.pos.y as f32).is_finite());
    assert!(result.alpha.is_finite());
}

#[test]
fn test_moffat_fit_wrong_background_estimate() {
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

    // Use wrong background estimate (20% error)
    let wrong_bg = true_bg * 1.2;
    let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, wrong_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Should still converge - background is a fitted parameter
    assert!(result.converged);
    // Centroid should still be accurate
    assert!(
        (result.pos.x as f32 - true_cx).abs() < 0.1,
        "x error: {}",
        (result.pos.x as f32 - true_cx).abs()
    );
    assert!(
        (result.pos.y as f32 - true_cy).abs() < 0.1,
        "y error: {}",
        (result.pos.y as f32 - true_cy).abs()
    );
    // Fitted background should be close to true value
    assert!(
        (result.background - true_bg).abs() < 0.05,
        "bg error: {}",
        (result.background - true_bg).abs()
    );
}

#[test]
fn test_moffat_fit_wrong_beta_still_finds_centroid() {
    let width = 21;
    let height = 21;
    let true_cx = 10.3;
    let true_cy = 10.7;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_beta = 4.0; // True beta
    let true_bg = 0.1;

    let pixels = make_moffat_stamp(
        width, height, true_cx, true_cy, true_amp, true_alpha, true_beta, true_bg,
    );
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: 2.5, // Wrong beta (2.5 instead of 4.0)
        ..Default::default()
    };
    let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    // Centroid should still be reasonably accurate even with wrong beta
    assert!(
        (result.pos.x as f32 - true_cx).abs() < 0.15,
        "x error: {}",
        (result.pos.x as f32 - true_cx).abs()
    );
    assert!(
        (result.pos.y as f32 - true_cy).abs() < 0.15,
        "y error: {}",
        (result.pos.y as f32 - true_cy).abs()
    );
}

// ============================================================================
// Amplitude and parameter edge cases
// ============================================================================

#[test]
fn test_moffat_fit_very_high_amplitude() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 10000.0; // Very high amplitude
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 100.0;

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
    assert!((result.pos.x as f32 - true_cx).abs() < 0.1);
    assert!((result.pos.y as f32 - true_cy).abs() < 0.1);
    assert!((result.amplitude - true_amp).abs() / true_amp < 0.01);
}

#[test]
fn test_moffat_fit_very_low_amplitude() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 0.01; // Very low amplitude
    let true_alpha = 2.5;
    let true_beta = 2.5;
    let true_bg = 0.001;

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
    assert!((result.pos.x as f32 - true_cx).abs() < 0.1);
    assert!((result.pos.y as f32 - true_cy).abs() < 0.1);
}

#[test]
fn test_moffat_fit_narrow_psf() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_alpha = 0.8; // Very narrow PSF
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
    assert!((result.pos.x as f32 - true_cx).abs() < 0.1);
    assert!((result.pos.y as f32 - true_cy).abs() < 0.1);
}

#[test]
fn test_moffat_fit_wide_psf() {
    let width = 31;
    let height = 31;
    let true_cx = 15.0;
    let true_cy = 15.0;
    let true_amp = 1.0;
    let true_alpha = 6.0; // Wide PSF
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
    let result = fit_moffat_2d(&pixels_buf, 15.0, 15.0, 12, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!((result.pos.x as f32 - true_cx).abs() < 0.1);
    assert!((result.pos.y as f32 - true_cy).abs() < 0.1);
    assert!(
        (result.alpha - true_alpha).abs() < 0.5,
        "alpha error: {}",
        (result.alpha - true_alpha).abs()
    );
}

#[test]
fn test_moffat_fit_various_beta_values() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_alpha = 2.5;
    let true_bg = 0.1;

    // Test various beta values from Lorentzian-like (1.5) to Gaussian-like (6.0)
    for &true_beta in &[1.5f32, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0] {
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

        assert!(result.is_some(), "Failed for beta={}", true_beta);
        let result = result.unwrap();
        assert!(
            result.converged,
            "Failed to converge for beta={}",
            true_beta
        );
        assert!(
            (result.pos.x as f32 - true_cx).abs() < 0.1,
            "beta={}: x error={}",
            true_beta,
            (result.pos.x as f32 - true_cx).abs()
        );
    }
}

// ============================================================================
// Convergence and iteration tests
// ============================================================================

#[test]
fn test_moffat_fit_converges_within_max_iterations() {
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
        lm: LMConfig {
            max_iterations: 10, // Low iteration limit
            ..Default::default()
        },
    };
    let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Should converge quickly for perfect data
    assert!(result.converged);
    assert!(result.iterations <= 10);
}

#[test]
fn test_moffat_fit_bad_initial_guess_still_converges() {
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
        lm: LMConfig {
            max_iterations: 100,
            ..Default::default()
        },
    };

    // Start from a position offset by 2 pixels
    let result = fit_moffat_2d(&pixels_buf, 8.0, 12.0, 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!(
        (result.pos.x as f32 - true_cx).abs() < 0.1,
        "x error: {}",
        (result.pos.x as f32 - true_cx).abs()
    );
    assert!(
        (result.pos.y as f32 - true_cy).abs() < 0.1,
        "y error: {}",
        (result.pos.y as f32 - true_cy).abs()
    );
}

#[test]
fn test_moffat_fit_uniform_data_returns_result() {
    // Uniform data (no star) - should still return a result, though meaningless
    let width = 21;
    let height = 21;
    let uniform_value = 0.5f32;
    let pixels = vec![uniform_value; width * height];
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = MoffatFitConfig::default();
    let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, uniform_value, &config);

    // Should produce some result (may not converge well)
    assert!(result.is_some());
    let result = result.unwrap();
    // Values should be finite
    assert!((result.pos.x as f32).is_finite());
    assert!((result.pos.y as f32).is_finite());
    assert!(result.amplitude.is_finite());
}

// ============================================================================
// FWHM computation tests
// ============================================================================

#[test]
fn test_moffat_fwhm_computed_correctly() {
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
    let result = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, true_bg, &config).unwrap();

    // FWHM should match analytical formula
    let expected_fwhm = alpha_beta_to_fwhm(true_alpha, true_beta);
    assert!(
        (result.fwhm - expected_fwhm).abs() < 0.2,
        "FWHM error: {} vs expected {}",
        result.fwhm,
        expected_fwhm
    );
}

#[test]
fn test_fwhm_increases_with_alpha() {
    let beta = 2.5;
    let fwhm1 = alpha_beta_to_fwhm(1.0, beta);
    let fwhm2 = alpha_beta_to_fwhm(2.0, beta);
    let fwhm3 = alpha_beta_to_fwhm(3.0, beta);

    assert!(fwhm2 > fwhm1);
    assert!(fwhm3 > fwhm2);
    // Should be linear with alpha
    assert!((fwhm2 / fwhm1 - 2.0).abs() < 0.01);
    assert!((fwhm3 / fwhm1 - 3.0).abs() < 0.01);
}

#[test]
fn test_fwhm_decreases_with_beta() {
    let alpha = 2.0;
    let fwhm_low_beta = alpha_beta_to_fwhm(alpha, 1.5);
    let fwhm_mid_beta = alpha_beta_to_fwhm(alpha, 2.5);
    let fwhm_high_beta = alpha_beta_to_fwhm(alpha, 5.0);

    // Higher beta = narrower profile = smaller FWHM
    assert!(fwhm_high_beta < fwhm_mid_beta);
    assert!(fwhm_mid_beta < fwhm_low_beta);
}
