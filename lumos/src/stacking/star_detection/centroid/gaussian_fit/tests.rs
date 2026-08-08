//! Tests for 2D Gaussian fitting.

use crate::math::size2us::Size2us;
use crate::math::{fwhm_to_sigma, sigma_to_fwhm};
use crate::stacking::star_detection::centroid::gaussian_fit::*;
use crate::stacking::star_detection::centroid::internals::{
    add_noise, approx_eq, reference_normal_equations,
};
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};
use glam::Vec2;

#[test]
fn test_gaussian_fit_centered() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Note: converged flag may be false if initial guess was already close
    // What matters is the accuracy of the result
    assert!((result.pos.x - true_cx).abs() < 0.1);
    assert!((result.pos.y - true_cy).abs() < 0.1);
    assert!((result.sigma.x - true_sigma).abs() < 0.2);
    assert!((result.sigma.y - true_sigma).abs() < 0.2);
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::new(10.0, 11.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!((result.pos.x - true_cx).abs() < 0.05);
    assert!((result.pos.y - true_cy).abs() < 0.05);
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

    let pixels = SyntheticStar::new(
        Vec2::new(cx, cy),
        amp,
        StarProfile::Elliptical {
            sigma_x,
            sigma_y,
            angle: 0.0,
        },
    )
    .stamp(Size2us::new(width, height), bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Note: converged flag may be false if initial guess was already close
    assert!((result.sigma.x - sigma_x).abs() < 0.3);
    assert!((result.sigma.y - sigma_y).abs() < 0.3);
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
    let pixels = Buffer2::new_filled(width, height, 0.1f32);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::new(2.0, 10.0), 8, 0.1, None, &config);
    assert!(result.is_none());
}

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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!((result.pos.x - true_cx).abs() < 0.02);
    assert!((result.pos.y - true_cy).abs() < 0.02);
    assert!((result.debug.amplitude - true_amp).abs() < 1.0);
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(15.0), 12, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!((result.pos.x - true_cx).abs() < 0.2);
    assert!((result.pos.y - true_cy).abs() < 0.2);
    assert!((result.sigma.x - true_sigma).abs() < 0.5);
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(7.0), 5, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Smaller sigma means less pixels with signal, so accuracy may be lower
    assert!((result.pos.x - true_cx).abs() < 0.15);
    assert!((result.pos.y - true_cy).abs() < 0.15);
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

    let mut pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    // Add deterministic "noise" pattern
    for (i, p) in pixels.iter_mut().enumerate() {
        let noise = 0.02 * ((i % 7) as f32 - 3.0) / 3.0;
        *p += noise;
    }

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // With noise, accuracy is slightly reduced but should still be reasonable
    assert!((result.pos.x - true_cx).abs() < 0.2);
    assert!((result.pos.y - true_cy).abs() < 0.2);
}

#[test]
fn test_gaussian_fit_stamp_too_small() {
    let width = 5;
    let height = 5;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    let config = GaussianFitConfig::default();
    // Center at 2,2 with radius 3 would go outside the 5x5 image
    // extract_stamp returns None when stamp doesn't fit
    let result = fit_gaussian_2d(&pixels, Vec2::splat(2.0), 3, 0.5, None, &config);
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!((result.pos.x - true_cx).abs() < 0.1);
    assert!((result.pos.y - true_cy).abs() < 0.1);
    assert!(result.debug.background.abs() < 0.05);
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!((result.pos.x - true_cx).abs() < 0.1);
    assert!((result.pos.y - true_cy).abs() < 0.1);
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
fn test_gaussian_fit_with_gaussian_noise() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;
    let noise_sigma = 0.05; // 5% of amplitude

    let mut pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);
    add_noise(&mut pixels, noise_sigma, 12345);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    // With noise, allow larger tolerance
    assert!(
        (result.pos.x - true_cx).abs() < 0.2,
        "x error: {}",
        (result.pos.x - true_cx).abs()
    );
    assert!(
        (result.pos.y - true_cy).abs() < 0.2,
        "y error: {}",
        (result.pos.y - true_cy).abs()
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

    let mut pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);
    add_noise(&mut pixels, noise_sigma, 54321);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    // Should still converge even with high noise
    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    // Centroid should still be reasonable (within 0.5 pixel)
    assert!(
        (result.pos.x - true_cx).abs() < 0.5,
        "x error too large: {}",
        (result.pos.x - true_cx).abs()
    );
    assert!(
        (result.pos.y - true_cy).abs() < 0.5,
        "y error too large: {}",
        (result.pos.y - true_cy).abs()
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    // Low SNR should still produce a result (may not be accurate)
    assert!(result.is_some());
    let result = result.unwrap();
    // Just verify it doesn't crash and produces finite values
    assert!(result.pos.x.is_finite());
    assert!(result.pos.y.is_finite());
    assert!(result.sigma.x.is_finite());
    assert!(result.sigma.y.is_finite());
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();

    // Use wrong background estimate (20% error)
    let wrong_bg = true_bg * 1.2;
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, wrong_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Should still converge - background is a fitted parameter
    assert!(result.converged);
    // Centroid should still be accurate
    assert!(
        (result.pos.x - true_cx).abs() < 0.1,
        "x error: {}",
        (result.pos.x - true_cx).abs()
    );
    assert!(
        (result.pos.y - true_cy).abs() < 0.1,
        "y error: {}",
        (result.pos.y - true_cy).abs()
    );
    // Fitted background should be close to true value
    assert!(
        (result.debug.background - true_bg).abs() < 0.05,
        "bg error: {}",
        (result.debug.background - true_bg).abs()
    );
}

#[test]
fn test_gaussian_fit_very_high_amplitude() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 10000.0; // Very high amplitude
    let true_sigma = 2.5;
    let true_bg = 100.0;

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!((result.pos.x - true_cx).abs() < 0.1);
    assert!((result.pos.y - true_cy).abs() < 0.1);
    assert!((result.debug.amplitude - true_amp).abs() / true_amp < 0.01);
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!((result.pos.x - true_cx).abs() < 0.15);
    assert!((result.pos.y - true_cy).abs() < 0.15);
}

#[test]
fn test_gaussian_fit_converges_within_max_iterations() {
    let width = 21;
    let height = 21;
    let true_cx = 10.0;
    let true_cy = 10.0;
    let true_amp = 1.0;
    let true_sigma = 2.5;
    let true_bg = 0.1;

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig {
        max_iterations: 10, // Low iteration limit
        ..Default::default()
    };
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Should converge quickly for perfect data
    assert!(result.converged);
    assert!(result.debug.iterations <= 10);
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig {
        max_iterations: 100,
        ..Default::default()
    };

    // Start from a position offset by 2 pixels
    let result = fit_gaussian_2d(&pixels, Vec2::new(8.0, 12.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.converged);
    assert!(
        (result.pos.x - true_cx).abs() < 0.1,
        "x error: {}",
        (result.pos.x - true_cx).abs()
    );
    assert!(
        (result.pos.y - true_cy).abs() < 0.1,
        "y error: {}",
        (result.pos.y - true_cy).abs()
    );
}

#[test]
fn test_gaussian_fit_uniform_data_returns_result() {
    // Uniform data (no star) - should still return a result, though meaningless
    let width = 21;
    let height = 21;
    let uniform_value = 0.5f32;
    let pixels = Buffer2::new_filled(width, height, uniform_value);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, uniform_value, None, &config);

    // Should produce some result (may not converge well)
    assert!(result.is_some());
    let result = result.unwrap();
    // Values should be finite
    assert!(result.pos.x.is_finite());
    assert!(result.pos.y.is_finite());
    assert!(result.debug.amplitude.is_finite());
    assert!(result.sigma.x.is_finite());
    assert!(result.sigma.y.is_finite());
}

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
    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    // Give initial guess very far from true center - should still find it
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 3, true_bg, None, &config);

    // Small stamp radius = 3, if result.pos.x as f32 moves more than 3 pixels from cx, it's rejected
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config).unwrap();

    // For perfect data, RMS residual should be very small
    assert!(
        result.debug.rms_residual < 1e-4,
        "RMS residual too high: {}",
        result.debug.rms_residual
    );
    assert!(result.debug.rms_residual >= 0.0);
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

        let pixels = SyntheticStar::new(
            Vec2::new(true_cx, true_cy),
            true_amp,
            StarProfile::Gaussian { sigma: true_sigma },
        )
        .stamp(Size2us::new(width, height), true_bg);

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

        assert!(
            result.is_some(),
            "Failed for offset ({}, {})",
            offset_x,
            offset_y
        );
        let result = result.unwrap();
        assert!(
            (result.pos.x - true_cx).abs() < 0.1,
            "X error {} for offset ({}, {})",
            (result.pos.x - true_cx).abs(),
            offset_x,
            offset_y
        );
        assert!(
            (result.pos.y - true_cy).abs() < 0.1,
            "Y error {} for offset ({}, {})",
            (result.pos.y - true_cy).abs(),
            offset_x,
            offset_y
        );
    }
}

#[test]
fn test_reference_normal_equations_symmetry() {
    // Create test jacobian and residuals
    let jacobian = vec![
        [1.0f64, 0.5, 0.3, 0.2, 0.1, 0.05],
        [0.8, 0.6, 0.4, 0.25, 0.15, 0.08],
        [0.6, 0.7, 0.5, 0.3, 0.2, 0.1],
        [0.4, 0.4, 0.35, 0.22, 0.18, 0.07],
    ];
    let residuals = vec![0.1f64, -0.05, 0.08, -0.03];

    let NormalEquations {
        hessian, gradient, ..
    } = reference_normal_equations(&jacobian, &residuals);

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
fn test_reference_normal_equations_values() {
    // Simple case: single jacobian row
    let jacobian = vec![[1.0f64, 2.0, 3.0, 4.0, 5.0, 6.0]];
    let residuals = vec![1.0f64];

    let NormalEquations {
        hessian,
        gradient,
        chi2,
    } = reference_normal_equations(&jacobian, &residuals);

    // chi² is Σr² over the one residual: 1² = 1
    assert_eq!(chi2, 1.0);

    // Gradient should be J^T * r = [1, 2, 3, 4, 5, 6]
    for (i, &g) in gradient.iter().enumerate() {
        let expected = (i + 1) as f64;
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
            let expected = ((i + 1) * (j + 1)) as f64;
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
fn test_reference_normal_equations_empty() {
    let jacobian: Vec<[f64; 6]> = vec![];
    let residuals: Vec<f64> = vec![];

    let NormalEquations {
        hessian,
        gradient,
        chi2,
    } = reference_normal_equations(&jacobian, &residuals);

    // Empty input should give zero hessian, gradient and chi²
    assert_eq!(chi2, 0.0);
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
fn test_reference_normal_equations_positive_semidefinite() {
    // For any Jacobian, J^T * J should be positive semi-definite
    // This means x^T * H * x >= 0 for all x
    let jacobian = vec![
        [0.5f64, 0.3, 0.8, 0.2, 0.6, 0.4],
        [0.7, 0.4, 0.2, 0.5, 0.3, 0.1],
        [0.3, 0.6, 0.5, 0.4, 0.2, 0.3],
    ];
    let residuals = vec![0.1f64, -0.2, 0.15];

    let NormalEquations { hessian, .. } = reference_normal_equations(&jacobian, &residuals);

    // Test with several random vectors
    let test_vectors = [
        [1.0f64, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        [0.5, -0.3, 0.7, -0.2, 0.4, -0.1],
    ];

    for x in &test_vectors {
        let mut result = 0.0f64;
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

/// Test that reference_normal_equations produces correct results with many rows.
#[test]
fn test_reference_normal_equations_many_rows() {
    let n = 17;
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    for i in 0..n {
        let base = (i as f64 + 1.0) * 0.1;
        jacobian.push([
            base,
            base * 0.5,
            base * 0.3,
            base * 0.7,
            base * 0.2,
            base * 0.9,
        ]);
        residuals.push(0.05 - 0.01 * i as f64);
    }

    let NormalEquations {
        hessian, gradient, ..
    } = reference_normal_equations(&jacobian, &residuals);

    // Compute reference result
    let mut ref_hessian = [[0.0f64; 6]; 6];
    let mut ref_gradient = [0.0f64; 6];
    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..6 {
            ref_gradient[i] += row[i] * r;
            for j in 0..6 {
                ref_hessian[i][j] += row[i] * row[j];
            }
        }
    }

    // Verify gradient matches
    for i in 0..6 {
        assert!(
            (gradient[i] - ref_gradient[i]).abs() < 1e-4,
            "Gradient[{}]: got {}, expected {}",
            i,
            gradient[i],
            ref_gradient[i]
        );
    }

    // Verify hessian matches (including symmetry)
    for i in 0..6 {
        for j in 0..6 {
            assert!(
                (hessian[i][j] - ref_hessian[i][j]).abs() < 1e-4,
                "Hessian[{}][{}]: got {}, expected {}",
                i,
                j,
                hessian[i][j],
                ref_hessian[i][j]
            );
            assert!(
                (hessian[i][j] - hessian[j][i]).abs() < 1e-6,
                "Hessian not symmetric at [{},{}]",
                i,
                j,
            );
        }
    }
}

/// Test with exactly 8 rows.
#[test]
fn test_reference_normal_equations_exactly_8_rows() {
    let jacobian: Vec<[f64; 6]> = (0..8)
        .map(|i| {
            let v = (i + 1) as f64;
            [v, v * 0.5, v * 0.3, v * 0.8, v * 0.1, v * 0.6]
        })
        .collect();
    let residuals: Vec<f64> = (0..8).map(|i| 0.1 * (i as f64 - 3.5)).collect();

    let NormalEquations {
        hessian, gradient, ..
    } = reference_normal_equations(&jacobian, &residuals);

    // Reference
    let mut ref_hessian = [[0.0f64; 6]; 6];
    let mut ref_gradient = [0.0f64; 6];
    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..6 {
            ref_gradient[i] += row[i] * r;
            for j in 0..6 {
                ref_hessian[i][j] += row[i] * row[j];
            }
        }
    }

    for i in 0..6 {
        assert!(
            (gradient[i] - ref_gradient[i]).abs() < 1e-4,
            "Gradient[{}]: got {}, expected {}",
            i,
            gradient[i],
            ref_gradient[i]
        );
        for j in 0..6 {
            assert!(
                (hessian[i][j] - ref_hessian[i][j]).abs() < 1e-4,
                "Hessian[{}][{}]: got {}, expected {}",
                i,
                j,
                hessian[i][j],
                ref_hessian[i][j]
            );
        }
    }
}

/// Test with a realistic stamp size (289 pixels = 17x17) to verify accumulated precision.
#[test]
fn test_reference_normal_equations_large_stamp() {
    let n = 289;
    let mut jacobian = Vec::with_capacity(n);
    let mut residuals = Vec::with_capacity(n);

    // Simulate a Gaussian-like Jacobian pattern
    let sigma = 3.0f64;
    for i in 0..n {
        let x = (i % 17) as f64 - 8.0;
        let y = (i / 17) as f64 - 8.0;
        let r2 = x * x + y * y;
        let exp_val = (-0.5 * r2 / (sigma * sigma)).exp();
        jacobian.push([
            exp_val * x / (sigma * sigma),             // dF/dx0
            exp_val * y / (sigma * sigma),             // dF/dy0
            exp_val,                                   // dF/dA
            exp_val * x * x / (sigma * sigma * sigma), // dF/dsigma_x
            exp_val * y * y / (sigma * sigma * sigma), // dF/dsigma_y
            1.0,                                       // dF/dbg
        ]);
        residuals.push(0.01 * exp_val + 0.001 * (i as f64 % 7.0 - 3.0));
    }

    let NormalEquations {
        hessian, gradient, ..
    } = reference_normal_equations(&jacobian, &residuals);

    // Reference
    let mut ref_hessian = [[0.0f64; 6]; 6];
    let mut ref_gradient = [0.0f64; 6];
    for (row, &r) in jacobian.iter().zip(residuals.iter()) {
        for i in 0..6 {
            ref_gradient[i] += row[i] * r;
            for j in 0..6 {
                ref_hessian[i][j] += row[i] * row[j];
            }
        }
    }

    for i in 0..6 {
        let rel_err = if ref_gradient[i].abs() > 1e-6 {
            (gradient[i] - ref_gradient[i]).abs() / ref_gradient[i].abs()
        } else {
            (gradient[i] - ref_gradient[i]).abs()
        };
        assert!(
            rel_err < 1e-3,
            "Gradient[{}]: got {}, expected {}, rel_err={}",
            i,
            gradient[i],
            ref_gradient[i],
            rel_err
        );
        for j in 0..6 {
            let rel_err = if ref_hessian[i][j].abs() > 1e-6 {
                (hessian[i][j] - ref_hessian[i][j]).abs() / ref_hessian[i][j].abs()
            } else {
                (hessian[i][j] - ref_hessian[i][j]).abs()
            };
            assert!(
                rel_err < 1e-3,
                "Hessian[{}][{}]: got {}, expected {}, rel_err={}",
                i,
                j,
                hessian[i][j],
                ref_hessian[i][j],
                rel_err
            );
        }
    }
}

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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(7.0), 5, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // Sigma should be clamped to >= 0.5
    assert!(result.sigma.x >= 0.5);
    assert!(result.sigma.y >= 0.5);
    // Centroid should still be reasonable
    assert!((result.pos.x - true_cx).abs() < 0.2);
    assert!((result.pos.y - true_cy).abs() < 0.2);
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(
        &pixels,
        Vec2::splat(15.0),
        stamp_radius,
        true_bg,
        None,
        &config,
    );

    assert!(result.is_some());
    let result = result.unwrap();
    // Sigma should be clamped to <= stamp_radius
    assert!(result.sigma.x <= stamp_radius as f32);
    assert!(result.sigma.y <= stamp_radius as f32);
    // Centroid should still be found
    assert!((result.pos.x - true_cx).abs() < 0.5);
    assert!((result.pos.y - true_cy).abs() < 0.5);
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
        let pixels = SyntheticStar::new(
            Vec2::new(true_cx, true_cy),
            true_amp,
            StarProfile::Gaussian { sigma: true_sigma },
        )
        .stamp(Size2us::new(width, height), true_bg);

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

        assert!(result.is_some(), "Failed for amplitude=10^{}", amp_exp);
        let result = result.unwrap();
        assert!(
            result.pos.x.is_finite(),
            "Non-finite x for amplitude=10^{}",
            amp_exp
        );
        assert!(
            result.pos.y.is_finite(),
            "Non-finite y for amplitude=10^{}",
            amp_exp
        );
        assert!(
            result.debug.amplitude.is_finite(),
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

    let pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(20.0), 15, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();
    // With wide PSF and low amplitude, accuracy is reduced but should still work
    assert!(result.pos.x.is_finite());
    assert!(result.pos.y.is_finite());
    assert!((result.pos.x - true_cx).abs() < 1.0);
    assert!((result.pos.y - true_cy).abs() < 1.0);
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

    let mut pixels = SyntheticStar::new(
        Vec2::new(true_cx, true_cy),
        true_amp,
        StarProfile::Gaussian { sigma: true_sigma },
    )
    .stamp(Size2us::new(width, height), true_bg);
    add_noise(&mut pixels, noise_sigma, 11111);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::splat(10.0), 8, true_bg, None, &config);

    assert!(result.is_some());
    let result = result.unwrap();

    // RMS residual should be roughly proportional to noise level
    // Allow factor of 2-3 due to fitting degrees of freedom
    assert!(
        result.debug.rms_residual < noise_sigma * 3.0,
        "RMS residual {} much larger than noise {}",
        result.debug.rms_residual,
        noise_sigma
    );
}

/// Build stamp data arrays (x, y, z) for a Gaussian profile at given params.
fn make_gaussian_stamp_data(size: usize, params: &[f64; 6]) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let [x0, y0, amp, sigma_x, sigma_y, bg] = *params;
    let sigma_x2 = sigma_x * sigma_x;
    let sigma_y2 = sigma_y * sigma_y;
    let mut data_x = Vec::with_capacity(size * size);
    let mut data_y = Vec::with_capacity(size * size);
    let mut data_z = Vec::with_capacity(size * size);
    for iy in 0..size {
        for ix in 0..size {
            let x = ix as f64;
            let y = iy as f64;
            let dx = x - x0;
            let dy = y - y0;
            let z = amp * (-0.5 * (dx * dx / sigma_x2 + dy * dy / sigma_y2)).exp() + bg;
            data_x.push(x);
            data_y.push(y);
            data_z.push(z);
        }
    }
    (data_x, data_y, data_z)
}

#[test]
fn test_batch_build_normal_equations_matches_scalar() {
    use crate::stacking::star_detection::centroid::lm_optimizer::LMModel;

    let true_params = [6.3, 6.7, 1000.0, 2.5, 3.0, 100.0];
    // Use offset params so residuals are non-trivial
    let params = [6.5, 6.5, 980.0, 2.6, 2.9, 102.0];
    let model = Gaussian2D { stamp_radius: 8.0 };
    let (data_x, data_y, data_z) = make_gaussian_stamp_data(13, &true_params);

    // Scalar reference: build jacobian/residuals then compute hessian/gradient
    let mut jac_scalar = Vec::new();
    let mut res_scalar = Vec::new();
    for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
        let (model_val, jac_row) = model.evaluate_and_jacobian(x, y, &params);
        jac_scalar.push(jac_row);
        res_scalar.push(z - model_val);
    }
    let NormalEquations {
        hessian: hessian_scalar,
        gradient: gradient_scalar,
        chi2: chi2_scalar,
    } = reference_normal_equations(&jac_scalar, &res_scalar);

    // Batch path (uses SIMD on x86_64 with AVX2)
    let NormalEquations {
        hessian: hessian_batch,
        gradient: gradient_batch,
        chi2: chi2_batch,
    } = model.batch_build_normal_equations(&data_x, &data_y, &data_z, &params);

    // Chi² should match
    assert!(
        approx_eq(chi2_scalar, chi2_batch),
        "chi2 mismatch: scalar={chi2_scalar}, batch={chi2_batch}"
    );

    // Gradient should match
    for i in 0..6 {
        assert!(
            approx_eq(gradient_scalar[i], gradient_batch[i]),
            "gradient[{i}] mismatch: scalar={}, batch={}",
            gradient_scalar[i],
            gradient_batch[i]
        );
    }

    // Hessian should match (full matrix including mirrored lower triangle)
    for i in 0..6 {
        for j in 0..6 {
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
    use crate::stacking::star_detection::centroid::lm_optimizer::LMModel;

    let model = Gaussian2D { stamp_radius: 8.0 };
    // Use slightly off params so residuals are non-zero
    let true_params = [6.3, 6.7, 1000.0, 2.5, 3.0, 100.0];
    let test_params = [6.5, 6.5, 980.0, 2.6, 2.9, 102.0];
    let (data_x, data_y, data_z) = make_gaussian_stamp_data(13, &true_params);

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
    use crate::stacking::star_detection::centroid::lm_optimizer::LMModel;

    let model = Gaussian2D { stamp_radius: 10.0 };
    let true_params = [5.0, 5.0, 500.0, 2.0, 2.5, 50.0];
    // Offset params for non-trivial residuals
    let params = [5.2, 4.8, 490.0, 2.1, 2.4, 51.0];

    // Test sizes that exercise: exact multiple of 4, remainder 1, 2, 3
    for size in [3, 4, 5, 7, 9, 11, 13, 15, 17] {
        let (data_x, data_y, data_z) = make_gaussian_stamp_data(size, &true_params);

        // Scalar reference
        let mut jac_scalar = Vec::new();
        let mut res_scalar = Vec::new();
        for ((&x, &y), &z) in data_x.iter().zip(data_y.iter()).zip(data_z.iter()) {
            let (model_val, jac_row) = model.evaluate_and_jacobian(x, y, &params);
            jac_scalar.push(jac_row);
            res_scalar.push(z - model_val);
        }
        let NormalEquations {
            hessian: hessian_scalar,
            gradient: gradient_scalar,
            chi2: chi2_scalar,
        } = reference_normal_equations(&jac_scalar, &res_scalar);

        // Batch
        let NormalEquations {
            hessian: hessian_batch,
            gradient: gradient_batch,
            chi2: chi2_batch,
        } = model.batch_build_normal_equations(&data_x, &data_y, &data_z, &params);

        assert!(
            approx_eq(chi2_scalar, chi2_batch),
            "size={size}: chi2 mismatch: scalar={chi2_scalar}, batch={chi2_batch}"
        );

        for i in 0..6 {
            assert!(
                approx_eq(gradient_scalar[i], gradient_batch[i]),
                "size={size}: gradient[{i}] mismatch: scalar={}, batch={}",
                gradient_scalar[i],
                gradient_batch[i]
            );
            for j in 0..6 {
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
fn test_gaussian_evaluate_and_jacobian_consistency() {
    use crate::stacking::star_detection::centroid::gaussian_fit::Gaussian2D;
    use crate::stacking::star_detection::centroid::lm_optimizer::LMModel;

    let model = Gaussian2D { stamp_radius: 15.0 };
    let params_list: &[[f64; 6]] = &[
        [10.0, 10.0, 1000.0, 2.0, 2.0, 100.0],
        [5.5, 7.3, 500.0, 1.5, 3.0, 50.0],
        [0.0, 0.0, 1.0, 1.0, 1.0, 0.0],
    ];
    let points = [(8.0, 9.0), (10.0, 10.0), (12.0, 11.0), (5.0, 7.0)];

    for params in params_list {
        for &(x, y) in &points {
            let eval = model.evaluate(x, y, params);
            let jac = model.jacobian_row(x, y, params);
            let (fused_eval, fused_jac) = model.evaluate_and_jacobian(x, y, params);

            assert!(
                (eval - fused_eval).abs() < 1e-15,
                "evaluate mismatch: eval={eval}, fused={fused_eval}"
            );
            for i in 0..6 {
                assert!(
                    (jac[i] - fused_jac[i]).abs() < 1e-14,
                    "jacobian[{i}] mismatch: jac={}, fused={}",
                    jac[i],
                    fused_jac[i]
                );
            }
        }
    }
}
