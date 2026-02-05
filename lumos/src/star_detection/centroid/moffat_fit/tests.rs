//! Tests for Moffat profile fitting.

use super::*;
use crate::star_detection::centroid::lm_optimizer::LMConfig;
use glam::Vec2;

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::new(2.0, 10.0), 8, 0.1, &config);
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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, wrong_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(15.0), 12, true_bg, &config);

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
        let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

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
            max_iterations: 20, // Moderate iteration limit
            ..Default::default()
        },
    };
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Should converge quickly for perfect data
    assert!(result.converged);
    assert!(result.iterations <= 20);
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
    let result = fit_moffat_2d(&pixels_buf, Vec2::new(8.0, 12.0), 8, true_bg, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, uniform_value, &config);

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
    let result = fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, true_bg, &config).unwrap();

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

// ============================================================================
// PowStrategy and fast_pow_neg tests
// ============================================================================

#[test]
fn test_select_pow_strategy_integers() {
    for beta in [1.0, 2.0, 3.0, 4.0, 5.0] {
        let strategy = select_pow_strategy(beta);
        assert!(
            matches!(strategy, PowStrategy::Int { .. }),
            "beta={beta} should select Int strategy, got {strategy:?}"
        );
    }
}

#[test]
fn test_select_pow_strategy_half_integers() {
    for beta in [1.5, 2.5, 3.5, 4.5, 5.5] {
        let strategy = select_pow_strategy(beta);
        assert!(
            matches!(strategy, PowStrategy::HalfInt { .. }),
            "beta={beta} should select HalfInt strategy, got {strategy:?}"
        );
    }
}

#[test]
fn test_select_pow_strategy_general() {
    for beta in [2.3, 3.7, 1.1, std::f64::consts::PI] {
        let strategy = select_pow_strategy(beta);
        assert!(
            matches!(strategy, PowStrategy::General { .. }),
            "beta={beta} should select General strategy, got {strategy:?}"
        );
    }
}

#[test]
fn test_fast_pow_neg_accuracy_half_integers() {
    let u_values = [1.01, 1.1, 1.5, 2.0, 5.0, 10.0, 100.0];
    let betas = [1.5, 2.5, 3.5, 4.5, 5.5];

    for &beta in &betas {
        let strategy = select_pow_strategy(beta);
        for &u in &u_values {
            let fast = fast_pow_neg(u, strategy);
            let reference = u.powf(-beta);
            let rel_err = ((fast - reference) / reference).abs();
            assert!(
                rel_err < 1e-14,
                "fast_pow_neg(u={u}, beta={beta}) = {fast}, expected {reference}, rel_err={rel_err}"
            );
        }
    }
}

#[test]
fn test_fast_pow_neg_accuracy_integers() {
    let u_values = [1.01, 1.1, 2.0, 5.0, 10.0];
    let betas = [1.0, 2.0, 3.0, 4.0, 5.0];

    for &beta in &betas {
        let strategy = select_pow_strategy(beta);
        for &u in &u_values {
            let fast = fast_pow_neg(u, strategy);
            let reference = u.powf(-beta);
            let rel_err = ((fast - reference) / reference).abs();
            assert!(
                rel_err < 1e-14,
                "fast_pow_neg(u={u}, beta={beta}) = {fast}, expected {reference}, rel_err={rel_err}"
            );
        }
    }
}

#[test]
fn test_fast_pow_neg_general_fallback() {
    let beta = 2.3;
    let strategy = select_pow_strategy(beta);
    let u = 3.0;
    let fast = fast_pow_neg(u, strategy);
    let reference = u.powf(-beta);
    assert!(
        (fast - reference).abs() < 1e-15,
        "General fallback should be identical to powf"
    );
}

#[test]
fn test_int_pow_correctness() {
    let u = 2.5;
    assert!((int_pow(u, 0) - 1.0).abs() < 1e-15);
    assert!((int_pow(u, 1) - u).abs() < 1e-15);
    assert!((int_pow(u, 2) - u * u).abs() < 1e-15);
    assert!((int_pow(u, 3) - u * u * u).abs() < 1e-14);
    assert!((int_pow(u, 4) - u.powi(4)).abs() < 1e-13);
    assert!((int_pow(u, 5) - u.powi(5)).abs() < 1e-12);
    assert!((int_pow(u, 6) - u.powi(6)).abs() < 1e-11);
    assert!((int_pow(u, 10) - u.powi(10)).abs() < 1e-6);
}

// ============================================================================
// evaluate_and_jacobian fused method tests
// ============================================================================

#[test]
fn test_moffat_fixed_beta_evaluate_and_jacobian_consistency() {
    let params_list: &[[f64; 5]] = &[
        [10.0, 10.0, 1000.0, 2.0, 100.0],
        [5.5, 7.3, 500.0, 3.0, 50.0],
        [0.0, 0.0, 1.0, 1.0, 0.0],
    ];
    let points = [(8.0, 9.0), (10.0, 10.0), (12.0, 11.0), (5.0, 7.0)];

    for beta in [2.0, 2.5, 3.0, 3.5, 4.5] {
        let model = MoffatFixedBeta::new(15.0, beta);
        for params in params_list {
            for &(x, y) in &points {
                let eval = model.evaluate(x, y, params);
                let jac = model.jacobian_row(x, y, params);
                let (fused_eval, fused_jac) = model.evaluate_and_jacobian(x, y, params);

                assert!(
                    (eval - fused_eval).abs() < 1e-15,
                    "evaluate mismatch: beta={beta}, eval={eval}, fused={fused_eval}"
                );
                for i in 0..5 {
                    assert!(
                        (jac[i] - fused_jac[i]).abs() < 1e-14,
                        "jacobian[{i}] mismatch: beta={beta}, jac={}, fused={}",
                        jac[i],
                        fused_jac[i]
                    );
                }
            }
        }
    }
}

#[test]
fn test_moffat_variable_beta_evaluate_and_jacobian_consistency() {
    let model = MoffatVariableBeta { stamp_radius: 15.0 };
    let params_list: &[[f64; 6]] = &[
        [10.0, 10.0, 1000.0, 2.0, 2.5, 100.0],
        [5.5, 7.3, 500.0, 3.0, 3.5, 50.0],
        [0.0, 0.0, 1.0, 1.0, 4.0, 0.0],
    ];
    let points = [(8.0, 9.0), (10.0, 10.0), (12.0, 11.0)];

    for params in params_list {
        for &(x, y) in &points {
            // Note: evaluate uses u.powf(-beta), while evaluate_and_jacobian uses
            // (-beta * ln(u)).exp(). These are mathematically equivalent but differ
            // by ~1e-14 in floating point. Use relative tolerance.
            let eval = model.evaluate(x, y, params);
            let jac = model.jacobian_row(x, y, params);
            let (fused_eval, fused_jac) = model.evaluate_and_jacobian(x, y, params);

            let eval_tol = eval.abs().max(1e-15) * 1e-12;
            assert!(
                (eval - fused_eval).abs() < eval_tol,
                "evaluate mismatch: eval={eval}, fused={fused_eval}, diff={}",
                (eval - fused_eval).abs()
            );
            for i in 0..6 {
                let jac_tol = jac[i].abs().max(1e-15) * 1e-12;
                assert!(
                    (jac[i] - fused_jac[i]).abs() < jac_tol,
                    "jacobian[{i}] mismatch: jac={}, fused={}, diff={}",
                    jac[i],
                    fused_jac[i],
                    (jac[i] - fused_jac[i]).abs()
                );
            }
        }
    }
}

// ============================================================================
// SIMD batch method correctness tests
// ============================================================================

/// Approximate equality with combined absolute + relative tolerance.
/// Handles near-zero values where relative tolerance alone would be too tight.
fn approx_eq(a: f64, b: f64) -> bool {
    let abs_diff = (a - b).abs();
    // Absolute tolerance for values near zero
    if abs_diff < 1e-14 {
        return true;
    }
    // Relative tolerance for larger values
    let max_abs = a.abs().max(b.abs());
    abs_diff / max_abs < 1e-10
}

/// Build stamp data arrays (x, y, z) for a Moffat profile at given params.
fn make_stamp_data(size: usize, params: &[f64; 5], beta: f64) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let [x0, y0, amp, alpha, bg] = *params;
    let alpha2 = alpha * alpha;
    let mut data_x = Vec::with_capacity(size * size);
    let mut data_y = Vec::with_capacity(size * size);
    let mut data_z = Vec::with_capacity(size * size);
    for iy in 0..size {
        for ix in 0..size {
            let x = ix as f64;
            let y = iy as f64;
            let r2 = (x - x0).powi(2) + (y - y0).powi(2);
            let z = amp * (1.0 + r2 / alpha2).powf(-beta) + bg;
            data_x.push(x);
            data_y.push(y);
            data_z.push(z);
        }
    }
    (data_x, data_y, data_z)
}

/// Scalar reference for computing J^T J (hessian) and J^T r (gradient).
#[allow(clippy::needless_range_loop)]
fn compute_hessian_gradient<const N: usize>(
    jacobian: &[[f64; N]],
    residuals: &[f64],
) -> ([[f64; N]; N], [f64; N]) {
    let mut hessian = [[0.0f64; N]; N];
    let mut gradient = [0.0f64; N];
    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..N {
            gradient[i] += row[i] * r;
            for j in i..N {
                hessian[i][j] += row[i] * row[j];
            }
        }
    }
    for i in 1..N {
        for j in 0..i {
            hessian[i][j] = hessian[j][i];
        }
    }
    (hessian, gradient)
}

#[test]
fn test_batch_build_normal_equations_matches_scalar() {
    use super::super::lm_optimizer::LMModel;

    let beta = 2.5;
    let true_params = [6.3, 6.7, 1000.0, 2.5, 100.0];
    // Use offset params so residuals are non-trivial
    let params = [6.5, 6.5, 980.0, 2.6, 102.0];
    let model = MoffatFixedBeta::new(8.0, beta);
    let (data_x, data_y, data_z) = make_stamp_data(13, &true_params, beta);

    // Scalar reference: build jacobian/residuals then compute hessian/gradient
    let mut jac_scalar = Vec::new();
    let mut res_scalar = Vec::new();
    let mut chi2_scalar = 0.0f64;
    for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
        let (model_val, jac_row) = model.evaluate_and_jacobian(x, y, &params);
        let residual = z - model_val;
        chi2_scalar += residual * residual;
        jac_scalar.push(jac_row);
        res_scalar.push(residual);
    }
    let (hessian_scalar, gradient_scalar) = compute_hessian_gradient(&jac_scalar, &res_scalar);

    // Batch path (uses SIMD on x86_64 with AVX2)
    let (hessian_batch, gradient_batch, chi2_batch) =
        model.batch_build_normal_equations(&data_x, &data_y, &data_z, &params);

    // Chi² should match
    assert!(
        approx_eq(chi2_scalar, chi2_batch),
        "chi2 mismatch: scalar={chi2_scalar}, batch={chi2_batch}"
    );

    // Gradient should match
    for i in 0..5 {
        assert!(
            approx_eq(gradient_scalar[i], gradient_batch[i]),
            "gradient[{i}] mismatch: scalar={}, batch={}",
            gradient_scalar[i],
            gradient_batch[i]
        );
    }

    // Hessian should match (full matrix including mirrored lower triangle)
    for i in 0..5 {
        for j in 0..5 {
            assert!(
                approx_eq(hessian_scalar[i][j], hessian_batch[i][j]),
                "hessian[{i}][{j}] mismatch: scalar={}, batch={}",
                hessian_scalar[i][j],
                hessian_batch[i][j]
            );
        }
    }
}

#[test]
fn test_batch_compute_chi2_matches_scalar() {
    use super::super::lm_optimizer::LMModel;

    let beta = 2.5;
    let model = MoffatFixedBeta::new(8.0, beta);
    // Use slightly off params so residuals are non-zero
    let true_params = [6.3, 6.7, 1000.0, 2.5, 100.0];
    let test_params = [6.5, 6.5, 980.0, 2.6, 102.0];
    let (data_x, data_y, data_z) = make_stamp_data(13, &true_params, beta);

    // Scalar chi²
    let chi2_scalar: f64 = data_x
        .iter()
        .zip(data_y.iter())
        .zip(data_z.iter())
        .map(|((&x, &y), &z)| {
            let r = z - model.evaluate(x, y, &test_params);
            r * r
        })
        .sum();

    // Batch chi² (uses SIMD on x86_64 with AVX2)
    let chi2_batch = model.batch_compute_chi2(&data_x, &data_y, &data_z, &test_params);

    assert!(
        approx_eq(chi2_scalar, chi2_batch),
        "chi2 mismatch: scalar={chi2_scalar}, batch={chi2_batch}, diff={}",
        (chi2_scalar - chi2_batch).abs()
    );
}

#[test]
fn test_batch_build_normal_equations_various_stamp_sizes() {
    use super::super::lm_optimizer::LMModel;

    let beta = 2.5;
    let model = MoffatFixedBeta::new(10.0, beta);
    let true_params = [5.0, 5.0, 500.0, 2.0, 50.0];
    // Offset params for non-trivial residuals
    let params = [5.2, 4.8, 490.0, 2.1, 51.0];

    // Test sizes that exercise: exact multiple of 4, remainder 1, 2, 3
    for size in [3, 4, 5, 7, 9, 11, 13, 15, 17] {
        let (data_x, data_y, data_z) = make_stamp_data(size, &true_params, beta);

        // Scalar reference
        let mut jac_scalar = Vec::new();
        let mut res_scalar = Vec::new();
        let mut chi2_scalar = 0.0f64;
        for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
            let (model_val, jac_row) = model.evaluate_and_jacobian(x, y, &params);
            let residual = z - model_val;
            chi2_scalar += residual * residual;
            jac_scalar.push(jac_row);
            res_scalar.push(residual);
        }
        let (hessian_scalar, gradient_scalar) = compute_hessian_gradient(&jac_scalar, &res_scalar);

        // Batch
        let (hessian_batch, gradient_batch, chi2_batch) =
            model.batch_build_normal_equations(&data_x, &data_y, &data_z, &params);

        assert!(
            approx_eq(chi2_scalar, chi2_batch),
            "size={size}: chi2 mismatch: scalar={chi2_scalar}, batch={chi2_batch}"
        );

        for i in 0..5 {
            assert!(
                approx_eq(gradient_scalar[i], gradient_batch[i]),
                "size={size}: gradient[{i}] mismatch: scalar={}, batch={}",
                gradient_scalar[i],
                gradient_batch[i]
            );
            for j in 0..5 {
                assert!(
                    approx_eq(hessian_scalar[i][j], hessian_batch[i][j]),
                    "size={size}: hessian[{i}][{j}] mismatch: scalar={}, batch={}",
                    hessian_scalar[i][j],
                    hessian_batch[i][j]
                );
            }
        }
    }
}

#[test]
fn test_batch_build_normal_equations_all_pow_strategies() {
    use super::super::lm_optimizer::LMModel;

    let true_params = [6.5, 6.5, 800.0, 2.0, 80.0];
    // Offset params for non-trivial residuals
    let params = [6.7, 6.3, 790.0, 2.1, 82.0];

    // HalfInt: 2.5, 3.5; Int: 2.0, 3.0; General: 2.3
    for beta in [2.0, 2.3, 2.5, 3.0, 3.5] {
        let model = MoffatFixedBeta::new(8.0, beta);
        let (data_x, data_y, data_z) = make_stamp_data(13, &true_params, beta);

        // Scalar reference
        let mut jac_scalar = Vec::new();
        let mut res_scalar = Vec::new();
        let mut chi2_scalar = 0.0f64;
        for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
            let (model_val, jac_row) = model.evaluate_and_jacobian(x, y, &params);
            let residual = z - model_val;
            chi2_scalar += residual * residual;
            jac_scalar.push(jac_row);
            res_scalar.push(residual);
        }
        let (hessian_scalar, gradient_scalar) = compute_hessian_gradient(&jac_scalar, &res_scalar);

        // Batch
        let (hessian_batch, gradient_batch, chi2_batch) =
            model.batch_build_normal_equations(&data_x, &data_y, &data_z, &params);

        assert!(
            approx_eq(chi2_scalar, chi2_batch),
            "beta={beta}: chi2 mismatch: scalar={chi2_scalar}, batch={chi2_batch}"
        );

        for i in 0..5 {
            assert!(
                approx_eq(gradient_scalar[i], gradient_batch[i]),
                "beta={beta}: gradient[{i}] mismatch: scalar={}, batch={}",
                gradient_scalar[i],
                gradient_batch[i]
            );
            for j in 0..5 {
                assert!(
                    approx_eq(hessian_scalar[i][j], hessian_batch[i][j]),
                    "beta={beta}: hessian[{i}][{j}] mismatch: scalar={}, batch={}",
                    hessian_scalar[i][j],
                    hessian_batch[i][j]
                );
            }
        }
    }
}
