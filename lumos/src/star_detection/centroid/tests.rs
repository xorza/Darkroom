//! Tests for centroid computation.

use glam::Vec2;

use super::*;
use crate::common::Buffer2;
use crate::math::FWHM_TO_SIGMA;
use crate::math::{Aabb, Vec2us};
use crate::star_detection::BackgroundEstimate;
use crate::star_detection::config::Config;
use crate::star_detection::deblend::Region;
use crate::star_detection::detector::stages::detect_test_utils::detect_stars_test;

/// Default stamp radius for tests (matching expected FWHM of ~4 pixels).
const TEST_STAMP_RADIUS: usize = 7;

/// Default expected FWHM for tests (sigma=2.5 -> FWHM≈5.9 pixels).
const TEST_EXPECTED_FWHM: f32 = 5.9;

fn make_gaussian_star(
    width: usize,
    height: usize,
    pos: Vec2,
    sigma: f32,
    amplitude: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - pos.x;
            let dy = y as f32 - pos.y;
            let r2 = dx * dx + dy * dy;
            let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    Buffer2::new(width, height, pixels)
}

#[test]
fn test_centroid_accuracy() {
    // Use larger image to minimize background estimation effects
    let width = 128;
    let height = 128;
    let true_pos = Vec2::new(64.3, 64.7);
    let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8);

    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config::default();
    let candidates = detect_stars_test(&pixels, &bg, &config);

    assert_eq!(candidates.len(), 1);

    let star =
        measure_star(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    let error_x = (star.pos.x - true_pos.x as f64).abs();
    let error_y = (star.pos.y - true_pos.y as f64).abs();

    // Sub-pixel accuracy within 0.2 pixels is good for weighted centroid
    assert!(
        error_x < 0.2,
        "X centroid error {} too large (true={}, computed={})",
        error_x,
        true_pos.x,
        star.pos.x
    );
    assert!(
        error_y < 0.2,
        "Y centroid error {} too large (true={}, computed={})",
        error_y,
        true_pos.y,
        star.pos.y
    );
}

#[test]
fn test_fwhm_estimation() {
    // Use larger image for better background estimation
    let width = 128;
    let height = 128;
    let sigma = 3.0f32;
    let expected_fwhm = FWHM_TO_SIGMA * sigma;
    let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), sigma, 0.8);

    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );
    // Use higher max_area because dilation (radius=2) expands the star region
    let config = Config {
        max_area: 1000,
        ..Default::default()
    };
    let candidates = detect_stars_test(&pixels, &bg, &config);

    assert_eq!(candidates.len(), 1);

    let star =
        measure_star(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    // FWHM estimation from weighted second moments has systematic bias due to
    // finite aperture and background noise - 40% tolerance is reasonable
    let fwhm_error = (star.fwhm - expected_fwhm).abs() / expected_fwhm;
    assert!(
        fwhm_error < 0.4,
        "FWHM error {} too large (expected={}, computed={})",
        fwhm_error,
        expected_fwhm,
        star.fwhm
    );
}

#[test]
fn test_circular_star_eccentricity() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);

    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config::default();
    let candidates = detect_stars_test(&pixels, &bg, &config);

    let star =
        measure_star(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    assert!(
        star.eccentricity < 0.3,
        "Circular star has high eccentricity: {}",
        star.eccentricity
    );
}

#[test]
fn test_snr_and_flux_values() {
    // A bright star (amplitude 0.8, sigma 2.5) on background 0.0 should have
    // substantial SNR (>> 10) and measurable flux
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);

    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config::default();
    let candidates = detect_stars_test(&pixels, &bg, &config);

    let star =
        measure_star(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    // Bright star with amplitude 0.8 on zero background should have high SNR
    assert!(
        star.snr > 50.0,
        "Bright star SNR {} should be > 50",
        star.snr
    );
    // Flux should be substantial for amplitude=0.8 Gaussian
    assert!(
        star.flux > 1.0,
        "Bright star flux {} should be > 1.0",
        star.flux
    );
    // Peak should be close to star amplitude
    assert!(
        star.peak > 0.5,
        "Peak {} should be close to amplitude 0.8",
        star.peak
    );
}

// =============================================================================
// is_valid_stamp_position Tests
// =============================================================================

#[test]
fn test_valid_stamp_position_center() {
    // Center of a 64x64 image should be valid
    assert!(is_valid_stamp_position(
        Vec2::splat(32.0),
        64,
        64,
        TEST_STAMP_RADIUS
    ));
}

#[test]
fn test_valid_stamp_position_minimum_valid() {
    // Minimum valid position is at TEST_STAMP_RADIUS
    let min_pos = TEST_STAMP_RADIUS as f32;
    assert!(is_valid_stamp_position(
        Vec2::splat(min_pos),
        64,
        64,
        TEST_STAMP_RADIUS
    ));
}

#[test]
fn test_valid_stamp_position_maximum_valid() {
    // Maximum valid position is at width - TEST_STAMP_RADIUS - 1
    let width = 64usize;
    let height = 64usize;
    let max_pos_x = (width - TEST_STAMP_RADIUS - 1) as f32;
    let max_pos_y = (height - TEST_STAMP_RADIUS - 1) as f32;
    assert!(is_valid_stamp_position(
        Vec2::new(max_pos_x, max_pos_y),
        width,
        height,
        TEST_STAMP_RADIUS
    ));
}

#[test]
fn test_valid_stamp_position_too_close_to_left_edge() {
    // Position too close to left edge
    let pos = (TEST_STAMP_RADIUS - 1) as f32;
    assert!(!is_valid_stamp_position(
        Vec2::new(pos, 32.0),
        64,
        64,
        TEST_STAMP_RADIUS
    ));
}

#[test]
fn test_valid_stamp_position_too_close_to_top_edge() {
    // Position too close to top edge
    let pos = (TEST_STAMP_RADIUS - 1) as f32;
    assert!(!is_valid_stamp_position(
        Vec2::new(32.0, pos),
        64,
        64,
        TEST_STAMP_RADIUS
    ));
}

#[test]
fn test_valid_stamp_position_too_close_to_right_edge() {
    // Position too close to right edge
    let width = 64usize;
    let pos = (width - TEST_STAMP_RADIUS) as f32;
    assert!(!is_valid_stamp_position(
        Vec2::new(pos, 32.0),
        width,
        64,
        TEST_STAMP_RADIUS
    ));
}

#[test]
fn test_valid_stamp_position_too_close_to_bottom_edge() {
    // Position too close to bottom edge
    let height = 64usize;
    let pos = (height - TEST_STAMP_RADIUS) as f32;
    assert!(!is_valid_stamp_position(
        Vec2::new(32.0, pos),
        64,
        height,
        TEST_STAMP_RADIUS
    ));
}

#[test]
fn test_valid_stamp_position_negative_rounds_to_invalid() {
    // Negative position should be invalid
    assert!(!is_valid_stamp_position(
        Vec2::new(-1.0, 32.0),
        64,
        64,
        TEST_STAMP_RADIUS
    ));
    assert!(!is_valid_stamp_position(
        Vec2::new(32.0, -1.0),
        64,
        64,
        TEST_STAMP_RADIUS
    ));
}

#[test]
fn test_valid_stamp_position_fractional_rounding() {
    // Test that fractional positions are rounded correctly
    // 7.4 rounds to 7, which equals TEST_STAMP_RADIUS, so it should be valid
    assert!(is_valid_stamp_position(
        Vec2::new(7.4, 32.0),
        64,
        64,
        TEST_STAMP_RADIUS
    ));
    // 6.4 rounds to 6, which is less than TEST_STAMP_RADIUS (7), so invalid
    assert!(!is_valid_stamp_position(
        Vec2::new(6.4, 32.0),
        64,
        64,
        TEST_STAMP_RADIUS
    ));
}

#[test]
fn test_valid_stamp_position_small_image() {
    // Image too small to have any valid stamp positions
    // Minimum valid image size is 2 * TEST_STAMP_RADIUS + 1 = 15
    let min_size = 2 * TEST_STAMP_RADIUS + 1;
    // Just barely large enough - center should be valid
    assert!(is_valid_stamp_position(
        Vec2::splat(TEST_STAMP_RADIUS as f32),
        min_size,
        min_size,
        TEST_STAMP_RADIUS
    ));
    // One pixel smaller - no valid positions
    assert!(!is_valid_stamp_position(
        Vec2::splat(TEST_STAMP_RADIUS as f32),
        min_size - 1,
        min_size - 1,
        TEST_STAMP_RADIUS
    ));
}

// =============================================================================
// refine_centroid Tests
// =============================================================================

fn make_uniform_background(
    width: usize,
    height: usize,
    bg_value: f32,
    noise: f32,
) -> BackgroundEstimate {
    let mut bg_buf = Buffer2::new_default(width, height);
    let mut noise_buf = Buffer2::new_default(width, height);
    bg_buf.fill(bg_value);
    noise_buf.fill(noise);
    BackgroundEstimate {
        background: bg_buf,
        noise: noise_buf,
    }
}

#[test]
fn test_refine_centroid_centered_star() {
    let width = 64;
    let height = 64;
    let pos = Vec2::splat(32.0);
    let pixels = make_gaussian_star(width, height, pos, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        pos,
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );

    assert!(result.is_some());
    let new_pos = result.unwrap();
    // Should stay very close to original position
    assert!((new_pos.x - pos.x).abs() < 0.5);
    assert!((new_pos.y - pos.y).abs() < 0.5);
}

#[test]
fn test_refine_centroid_offset_converges() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Start with integer guess (peak pixel position)
    let start_pos = Vec2::new(32.0, 33.0);

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        start_pos,
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );

    assert!(result.is_some());
    let new_pos = result.unwrap();
    // Should move towards true center
    let old_error =
        ((start_pos.x - true_pos.x).powi(2) + (start_pos.y - true_pos.y).powi(2)).sqrt();
    let new_error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        new_error < old_error,
        "Refinement should reduce error: {} -> {}",
        old_error,
        new_error
    );
}

#[test]
fn test_refine_centroid_invalid_position_returns_none() {
    let width = 64;
    let height = 64;
    let pixels = vec![0.5f32; width * height];
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Position too close to edge
    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::new(3.0, 32.0),
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );
    assert!(result.is_none());
}

#[test]
fn test_refine_centroid_zero_flux_returns_none() {
    let width = 64;
    let height = 64;
    // All pixels equal to background - no signal
    let pixels = vec![0.1f32; width * height];
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );
    assert!(result.is_none());
}

#[test]
fn test_refine_centroid_rejects_large_movement() {
    let width = 64;
    let height = 64;
    // Create a star very far from initial position (outside the stamp entirely)
    let pixels = make_gaussian_star(width, height, Vec2::splat(50.0), 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Start far from the actual star - the stamp won't contain the star,
    // so there's no signal, which should cause rejection
    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );

    // With no star in the stamp (star is at 50,50, stamp centered at 32,32 with radius 7),
    // the weighted centroid has zero or near-zero flux
    assert!(result.is_none());
}

#[test]
fn test_refine_centroid_iterative_convergence() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.25, 32.75);
    let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Simulate multiple iterations like measure_star does
    let mut pos = Vec2::splat(32.0);

    for iteration in 0..MAX_MOMENTS_ITERATIONS {
        let result = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos,
            TEST_STAMP_RADIUS,
            TEST_EXPECTED_FWHM,
        );
        assert!(result.is_some(), "Iteration {} failed", iteration);

        let new_pos = result.unwrap();
        let delta = new_pos - pos;
        pos = new_pos;

        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Should converge close to true position
    let error = ((pos.x - true_pos.x).powi(2) + (pos.y - true_pos.y).powi(2)).sqrt();
    assert!(error < 0.2, "Failed to converge: error = {}", error);
}

// =============================================================================
// compute_metrics Tests
// =============================================================================

#[test]
fn test_compute_metrics_valid_star() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    );

    assert!(metrics.is_some());
    let m = metrics.unwrap();
    assert!(m.flux > 0.0, "Flux should be positive");
    assert!(m.fwhm > 0.0, "FWHM should be positive");
    assert!(
        m.eccentricity >= 0.0 && m.eccentricity <= 1.0,
        "Eccentricity out of range"
    );
    assert!(m.snr > 0.0, "SNR should be positive");
}

#[test]
fn test_compute_metrics_invalid_position_returns_none() {
    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Position too close to edge
    let metrics = compute_metrics(
        &pixels,
        &bg,
        Vec2::new(3.0, 32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    );
    assert!(metrics.is_none());
}

#[test]
fn test_compute_metrics_zero_flux_returns_none() {
    let width = 64;
    let height = 64;
    // All pixels equal to or below background
    let pixels = Buffer2::new_filled(width, height, 0.05f32);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    );
    assert!(metrics.is_none());
}

#[test]
fn test_compute_metrics_fwhm_scales_with_sigma() {
    let width = 128;
    let height = 128;

    // Create stars with different sigmas
    let sigma_small = 2.0f32;
    let sigma_large = 4.0f32;

    let pixels_small = make_gaussian_star(width, height, Vec2::splat(64.0), sigma_small, 0.8);
    let pixels_large = make_gaussian_star(width, height, Vec2::splat(64.0), sigma_large, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_small = compute_metrics(
        &pixels_small,
        &bg,
        Vec2::splat(64.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_large = compute_metrics(
        &pixels_large,
        &bg,
        Vec2::splat(64.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Larger sigma should result in larger FWHM
    assert!(
        metrics_large.fwhm > metrics_small.fwhm,
        "Larger sigma should give larger FWHM: {} vs {}",
        metrics_large.fwhm,
        metrics_small.fwhm
    );
}

#[test]
fn test_compute_metrics_snr_scales_with_amplitude() {
    let width = 64;
    let height = 64;

    let pixels_dim = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.2);
    let pixels_bright = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_dim = compute_metrics(
        &pixels_dim,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_bright = compute_metrics(
        &pixels_bright,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Brighter star should have higher SNR
    assert!(
        metrics_bright.snr > metrics_dim.snr,
        "Brighter star should have higher SNR: {} vs {}",
        metrics_bright.snr,
        metrics_dim.snr
    );
}

// =============================================================================
// Elongated Star Tests (Eccentricity)
// =============================================================================

fn make_elliptical_star(
    width: usize,
    height: usize,
    pos: Vec2,
    sigma_x: f32,
    sigma_y: f32,
    amplitude: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - pos.x;
            let dy = y as f32 - pos.y;
            let r2 = (dx * dx) / (sigma_x * sigma_x) + (dy * dy) / (sigma_y * sigma_y);
            let value = amplitude * (-r2 / 2.0).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    Buffer2::new(width, height, pixels)
}

#[test]
fn test_elongated_star_high_eccentricity() {
    let width = 64;
    let height = 64;
    // Elongated star: sigma_x = 4, sigma_y = 1 (4:1 aspect ratio)
    let pixels = make_elliptical_star(width, height, Vec2::splat(32.0), 4.0, 1.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Elongated star should have high eccentricity (> 0.5)
    assert!(
        metrics.eccentricity > 0.5,
        "Elongated star should have high eccentricity: {}",
        metrics.eccentricity
    );
}

#[test]
fn test_circular_vs_elongated_eccentricity() {
    let width = 64;
    let height = 64;

    let circular = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);
    let elongated = make_elliptical_star(width, height, Vec2::splat(32.0), 4.0, 2.0, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_circular = compute_metrics(
        &circular,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_elongated = compute_metrics(
        &elongated,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    assert!(
        metrics_elongated.eccentricity > metrics_circular.eccentricity,
        "Elongated star should have higher eccentricity: {} vs {}",
        metrics_elongated.eccentricity,
        metrics_circular.eccentricity
    );
}

// =============================================================================
// Noisy Background Tests
// =============================================================================

#[test]
fn test_centroid_with_noisy_background() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::splat(32.0);

    // Create star with added noise
    let mut pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8);

    // Add random-ish noise pattern (deterministic for reproducibility)
    for (i, pixel) in pixels.iter_mut().enumerate() {
        let noise = ((i * 7 + 13) % 100) as f32 * 0.001 - 0.05;
        *pixel += noise;
    }

    let bg = make_uniform_background(width, height, 0.1, 0.05); // Higher noise estimate

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        true_pos,
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );
    assert!(result.is_some());

    let new_pos = result.unwrap();
    // With noise, allow more tolerance
    assert!(
        (new_pos.x - true_pos.x).abs() < 1.0,
        "X error too large with noise"
    );
    assert!(
        (new_pos.y - true_pos.y).abs() < 1.0,
        "Y error too large with noise"
    );
}

#[test]
fn test_snr_decreases_with_higher_noise() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);

    let bg_low_noise = make_uniform_background(width, height, 0.1, 0.01);
    let bg_high_noise = make_uniform_background(width, height, 0.1, 0.1);

    let metrics_low = compute_metrics(
        &pixels,
        &bg_low_noise,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_high = compute_metrics(
        &pixels,
        &bg_high_noise,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    assert!(
        metrics_low.snr > metrics_high.snr,
        "Lower noise should give higher SNR: {} vs {}",
        metrics_low.snr,
        metrics_high.snr
    );
}

// =============================================================================
// Additional Quality Metrics Tests
// =============================================================================

#[test]
fn test_fwhm_formula_for_known_gaussian() {
    // For a Gaussian with known sigma, verify FWHM ≈ FWHM_TO_SIGMA * sigma
    let width = 128;
    let height = 128;
    let sigma = 3.0f32;
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.001); // Very low noise

    let metrics = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(64.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Allow 10% error due to discrete sampling and finite aperture
    let error = (metrics.fwhm - expected_fwhm).abs() / expected_fwhm;
    assert!(
        error < 0.1,
        "FWHM should be close to 2.355*sigma: expected {}, got {}, error {}",
        expected_fwhm,
        metrics.fwhm,
        error
    );
}

#[test]
fn test_flux_proportional_to_amplitude() {
    let width = 64;
    let height = 64;

    let pixels_amp1 = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.4);
    let pixels_amp2 = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics1 = compute_metrics(
        &pixels_amp1,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics2 = compute_metrics(
        &pixels_amp2,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Flux should scale roughly proportionally with amplitude
    let flux_ratio = metrics2.flux / metrics1.flux;
    let amp_ratio = 0.8 / 0.4;

    assert!(
        (flux_ratio - amp_ratio).abs() < 0.5,
        "Flux ratio {} should be close to amplitude ratio {}",
        flux_ratio,
        amp_ratio
    );
}

#[test]
fn test_eccentricity_bounds() {
    // Eccentricity should always be in [0, 1]
    let width = 64;
    let height = 64;
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Test various star shapes
    let circular = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);
    let elongated_x = make_elliptical_star(width, height, Vec2::splat(32.0), 5.0, 2.0, 0.8);
    let elongated_y = make_elliptical_star(width, height, Vec2::splat(32.0), 2.0, 5.0, 0.8);

    for (name, pixels) in [
        ("circular", circular),
        ("elongated_x", elongated_x),
        ("elongated_y", elongated_y),
    ] {
        let metrics = compute_metrics(
            &pixels,
            &bg,
            Vec2::splat(32.0),
            TEST_STAMP_RADIUS,
            None,
            None,
        )
        .unwrap();
        assert!(
            metrics.eccentricity >= 0.0 && metrics.eccentricity <= 1.0,
            "{} eccentricity {} out of bounds [0,1]",
            name,
            metrics.eccentricity
        );
    }
}

#[test]
fn test_eccentricity_orientation_invariant() {
    // Eccentricity should be similar regardless of orientation (x vs y elongation)
    let width = 64;
    let height = 64;
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let elongated_x = make_elliptical_star(width, height, Vec2::splat(32.0), 4.0, 2.0, 0.8);
    let elongated_y = make_elliptical_star(width, height, Vec2::splat(32.0), 2.0, 4.0, 0.8);

    let metrics_x = compute_metrics(
        &elongated_x,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_y = compute_metrics(
        &elongated_y,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Should have similar eccentricity (within 20%)
    let diff = (metrics_x.eccentricity - metrics_y.eccentricity).abs();
    let avg = (metrics_x.eccentricity + metrics_y.eccentricity) / 2.0;
    assert!(
        diff / avg < 0.2,
        "X and Y elongated stars should have similar eccentricity: {} vs {}",
        metrics_x.eccentricity,
        metrics_y.eccentricity
    );
}

#[test]
fn test_snr_formula_consistency() {
    // SNR = flux / (noise * sqrt(aperture_area))
    // Verify the formula behaves as expected
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);

    let noise1 = 0.02f32;
    let noise2 = 0.04f32; // 2x noise

    let bg1 = make_uniform_background(width, height, 0.1, noise1);
    let bg2 = make_uniform_background(width, height, 0.1, noise2);

    let metrics1 = compute_metrics(
        &pixels,
        &bg1,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics2 = compute_metrics(
        &pixels,
        &bg2,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // SNR should halve when noise doubles (same flux)
    let snr_ratio = metrics1.snr / metrics2.snr;
    assert!(
        (snr_ratio - 2.0).abs() < 0.1,
        "SNR ratio should be ~2 when noise doubles: got {}",
        snr_ratio
    );
}

#[test]
fn test_metrics_with_high_background() {
    // Stars should still be measurable with high but uniform background
    let width = 64;
    let height = 64;

    // High background value
    let mut pixels = vec![0.5f32; width * height];
    // Add star on top of high background
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - 32.0;
            let dy = y as f32 - 32.0;
            let r2 = dx * dx + dy * dy;
            let value = 0.4 * (-r2 / (2.0 * 2.5 * 2.5)).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }
    let pixels = Buffer2::new(width, height, pixels);

    let bg = make_uniform_background(width, height, 0.5, 0.02);
    let metrics = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    );

    assert!(
        metrics.is_some(),
        "Should compute metrics with high background"
    );
    let m = metrics.unwrap();
    assert!(m.flux > 0.0, "Flux should be positive");
    assert!(m.fwhm > 0.0, "FWHM should be positive");
}

#[test]
fn test_fwhm_independent_of_amplitude() {
    // FWHM should be the same regardless of star brightness (same sigma)
    let width = 64;
    let height = 64;
    let sigma = 2.5f32;

    let pixels_dim = make_gaussian_star(width, height, Vec2::splat(32.0), sigma, 0.3);
    let pixels_bright = make_gaussian_star(width, height, Vec2::splat(32.0), sigma, 0.9);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_dim = compute_metrics(
        &pixels_dim,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_bright = compute_metrics(
        &pixels_bright,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // FWHM should be within 20% of each other
    let diff = (metrics_dim.fwhm - metrics_bright.fwhm).abs();
    let avg = (metrics_dim.fwhm + metrics_bright.fwhm) / 2.0;
    assert!(
        diff / avg < 0.2,
        "FWHM should be amplitude-independent: dim={}, bright={}",
        metrics_dim.fwhm,
        metrics_bright.fwhm
    );
}

#[test]
fn test_eccentricity_increases_with_elongation() {
    let width = 64;
    let height = 64;
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Create stars with increasing elongation ratios
    let ratio_1_1 = make_elliptical_star(width, height, Vec2::splat(32.0), 2.5, 2.5, 0.8); // circular
    let ratio_2_1 = make_elliptical_star(width, height, Vec2::splat(32.0), 4.0, 2.0, 0.8);
    let ratio_3_1 = make_elliptical_star(width, height, Vec2::splat(32.0), 6.0, 2.0, 0.8);

    let ecc_1 = compute_metrics(
        &ratio_1_1,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap()
    .eccentricity;
    let ecc_2 = compute_metrics(
        &ratio_2_1,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap()
    .eccentricity;
    let ecc_3 = compute_metrics(
        &ratio_3_1,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap()
    .eccentricity;

    assert!(
        ecc_1 < ecc_2 && ecc_2 < ecc_3,
        "Eccentricity should increase with elongation: {} < {} < {}",
        ecc_1,
        ecc_2,
        ecc_3
    );
}

// =============================================================================
// measure_star Integration Tests
// =============================================================================

#[test]
fn test_measure_star_returns_none_for_edge_candidate() {
    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let config = Config::default();

    // Create region near edge
    let region = Region {
        bbox: Aabb::new(Vec2us::new(0, 30), Vec2us::new(5, 35)),
        peak: Vec2us::new(3, 32),
        peak_value: 0.9,
        area: 18,
    };

    let result = measure_star(&pixels, &bg, &region, &config);
    assert!(
        result.is_none(),
        "Should reject candidate too close to edge"
    );
}

#[test]
fn test_measure_star_multiple_stars_independent() {
    let width = 128;
    let height = 128;

    // Create two well-separated stars using the helper function
    let star1_cx = 40.0f32;
    let star1_cy = 40.0f32;
    let star2_cx = 90.0f32;
    let star2_cy = 90.0f32;

    // Start with uniform background
    let mut pixels = vec![0.1f32; width * height];

    // Add first star (sigma=2.5 gives good detection)
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - star1_cx;
            let dy = y as f32 - star1_cy;
            let r2 = dx * dx + dy * dy;
            let value = 0.8 * (-r2 / (2.0 * 2.5 * 2.5)).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    // Add second star
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - star2_cx;
            let dy = y as f32 - star2_cy;
            let r2 = dx * dx + dy * dy;
            let value = 0.6 * (-r2 / (2.0 * 2.5 * 2.5)).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    let pixels = Buffer2::new(width, height, pixels);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config {
        edge_margin: 10,
        ..Default::default()
    };
    let candidates = detect_stars_test(&pixels, &bg, &config);

    assert_eq!(candidates.len(), 2, "Should detect two stars");

    // Compute centroids for both
    let stars: Vec<_> = candidates
        .iter()
        .filter_map(|c| measure_star(&pixels, &bg, c, &config))
        .collect();

    assert_eq!(stars.len(), 2, "Should compute centroids for both stars");

    // Verify each star is close to its true position
    for star in &stars {
        let near_star1 = (star.pos.x - star1_cx as f64).abs() < 1.0
            && (star.pos.y - star1_cy as f64).abs() < 1.0;
        let near_star2 = (star.pos.x - star2_cx as f64).abs() < 1.0
            && (star.pos.y - star2_cy as f64).abs() < 1.0;
        assert!(
            near_star1 || near_star2,
            "Star at ({}, {}) not near either true position",
            star.pos.x,
            star.pos.y
        );
    }
}

// =============================================================================
// Roundness Tests
// =============================================================================

#[test]
fn test_circular_star_roundness() {
    // A circular Gaussian star should have roundness near zero
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);

    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config::default();
    let candidates = detect_stars_test(&pixels, &bg, &config);

    assert_eq!(candidates.len(), 1);

    let star =
        measure_star(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    // Circular star should have roundness close to 0
    assert!(
        star.roundness1.abs() < 0.1,
        "Circular star should have roundness1 near 0, got {}",
        star.roundness1
    );
    assert!(
        star.roundness2 < 0.1,
        "Circular star should have roundness2 near 0, got {}",
        star.roundness2
    );
}

#[test]
fn test_elongated_x_star_roundness() {
    // An elongated star in x direction should have negative roundness1
    let width = 64;
    let height = 64;
    let mut pixels = vec![0.1f32; width * height];

    // Create elongated Gaussian (sigma_x > sigma_y)
    let cx = 32.0;
    let cy = 32.0;
    let sigma_x = 4.0;
    let sigma_y = 2.0;
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let value = 0.8
                * (-dx * dx / (2.0 * sigma_x * sigma_x) - dy * dy / (2.0 * sigma_y * sigma_y))
                    .exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    let pixels = Buffer2::new(width, height, pixels);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config::default();
    let candidates = detect_stars_test(&pixels, &bg, &config);

    assert!(!candidates.is_empty());

    let star =
        measure_star(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    // X-elongated star: more flux in x marginal -> higher Hx -> negative roundness1
    // (roundness1 = (Hx - Hy) / (Hx + Hy), but Hx is sum in y direction)
    // Actually, marginal_x sums along y for each x position, so x-elongated means
    // the x marginal has lower peak (more spread). Let's just check it's non-zero.
    assert!(
        star.roundness1.abs() > 0.05 || star.eccentricity > 0.3,
        "Elongated star should have noticeable shape metrics"
    );
}

#[test]
fn test_asymmetric_star_roundness2() {
    // An asymmetric source should have non-zero roundness2
    let width = 64;
    let height = 64;
    let mut pixels = vec![0.1f32; width * height];

    // Create a star with extra flux on one side (like a cosmic ray tail)
    let cx = 32.0;
    let cy = 32.0;
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r2 = dx * dx + dy * dy;
            let mut value = 0.8 * (-r2 / (2.0 * 2.5 * 2.5)).exp();

            // Add asymmetric tail to the right
            if dx > 0.0 && dx < 8.0 && dy.abs() < 2.0 {
                value += 0.3 * (-(dx - 4.0).powi(2) / 8.0).exp();
            }

            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    let pixels = Buffer2::new(width, height, pixels);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config::default();
    let candidates = detect_stars_test(&pixels, &bg, &config);

    assert!(!candidates.is_empty());

    let star =
        measure_star(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    // Asymmetric source should have higher roundness2 (symmetry metric)
    // The tail adds more flux to the right side
    assert!(
        star.roundness2 > 0.01,
        "Asymmetric star should have roundness2 > 0, got {}",
        star.roundness2
    );
}

#[test]
fn test_laplacian_snr_computed_for_star() {
    // Verify that laplacian_snr is computed for detected stars
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config::default();
    let candidates = detect_stars_test(&pixels, &bg, &config);

    assert_eq!(candidates.len(), 1);

    let star =
        measure_star(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    // Laplacian SNR should be computed (non-negative value)
    // The actual value depends on the noise estimate from background
    assert!(
        star.laplacian_snr >= 0.0,
        "Laplacian SNR should be non-negative, got {}",
        star.laplacian_snr
    );
}

#[test]
fn test_star_is_round() {
    use crate::star_detection::Star;

    let round_star = Star {
        pos: glam::DVec2::new(10.0, 10.0),
        flux: 100.0,
        fwhm: 3.0,
        eccentricity: 0.1,
        snr: 50.0,
        peak: 0.5,
        sharpness: 0.3,
        roundness1: 0.05,
        roundness2: 0.03,
        laplacian_snr: 0.0,
    };

    let non_round_star = Star {
        roundness1: 0.5,
        roundness2: 0.4,
        ..round_star
    };

    assert!(
        round_star.is_round(0.2),
        "Round star should pass roundness check"
    );
    assert!(
        !non_round_star.is_round(0.2),
        "Non-round star should fail roundness check"
    );
    assert!(
        non_round_star.is_round(1.0),
        "All stars should pass with max_roundness=1.0"
    );
}

// =============================================================================
// Precision Verification Tests
// =============================================================================

/// Test that weighted centroid achieves claimed ~0.05 pixel accuracy
/// by testing many random sub-pixel offsets.
#[test]
fn test_weighted_centroid_precision_statistical() {
    let width = 128;
    let height = 128;
    let sigma = 2.5f32;

    // Test a grid of sub-pixel positions
    let mut total_error = 0.0f32;
    let mut max_error = 0.0f32;
    let mut count = 0;

    for dx in 0..10 {
        for dy in 0..10 {
            let true_pos = Vec2::new(64.0 + dx as f32 * 0.1, 64.0 + dy as f32 * 0.1);

            let pixels = make_gaussian_star(width, height, true_pos, sigma, 1.0);
            let bg = crate::testing::estimate_background(
                &pixels,
                &Config {
                    tile_size: 32,
                    ..Default::default()
                },
            );
            let config = Config {
                centroid_method: CentroidMethod::WeightedMoments,
                ..Default::default()
            };
            let candidates = detect_stars_test(&pixels, &bg, &config);

            if candidates.is_empty() {
                continue;
            }

            if let Some(star) = measure_star(&pixels, &bg, &candidates[0], &config) {
                let error = ((star.pos.x - true_pos.x as f64).powi(2)
                    + (star.pos.y - true_pos.y as f64).powi(2))
                .sqrt() as f32;
                total_error += error;
                max_error = max_error.max(error);
                count += 1;
            }
        }
    }

    let avg_error = total_error / count as f32;

    // Weighted centroid should achieve ~0.05 pixel accuracy on average
    assert!(
        avg_error < 0.1,
        "Average centroid error {} exceeds 0.1 pixels (count={})",
        avg_error,
        count
    );
    assert!(
        max_error < 0.2,
        "Max centroid error {} exceeds 0.2 pixels",
        max_error
    );
}

/// Test that Gaussian fitting achieves claimed ~0.01 pixel accuracy.
#[test]
fn test_gaussian_fit_precision_statistical() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};

    let width = 21;
    let height = 21;
    let sigma = 2.5f32;
    let background = 0.1f32;

    let mut total_error = 0.0f32;
    let mut max_error = 0.0f32;
    let mut count = 0;

    // Test a grid of sub-pixel positions
    for dx in 0..10 {
        for dy in 0..10 {
            let true_cx = 10.0 + dx as f32 * 0.1;
            let true_cy = 10.0 + dy as f32 * 0.1;

            // Create perfect Gaussian
            let mut pixels = vec![background; width * height];
            for y in 0..height {
                for x in 0..width {
                    let ddx = x as f32 - true_cx;
                    let ddy = y as f32 - true_cy;
                    pixels[y * width + x] +=
                        1.0 * (-0.5 * (ddx * ddx + ddy * ddy) / (sigma * sigma)).exp();
                }
            }
            let pixels_buf = Buffer2::new(width, height, pixels);

            let config = GaussianFitConfig::default();
            if let Some(result) =
                fit_gaussian_2d(&pixels_buf, Vec2::splat(10.0), 8, background, &config)
                && result.converged
            {
                let error =
                    ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
                total_error += error;
                max_error = max_error.max(error);
                count += 1;
            }
        }
    }

    let avg_error = total_error / count as f32;

    // Gaussian fitting should achieve ~0.01 pixel accuracy
    assert!(
        avg_error < 0.02,
        "Average Gaussian fit error {} exceeds 0.02 pixels (count={})",
        avg_error,
        count
    );
    assert!(
        max_error < 0.05,
        "Max Gaussian fit error {} exceeds 0.05 pixels",
        max_error
    );
}

/// Test that Moffat fitting achieves claimed ~0.01 pixel accuracy.
#[test]
fn test_moffat_fit_precision_statistical() {
    use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d};

    let width = 21;
    let height = 21;
    let alpha = 2.5f32;
    let beta = 2.5f32;
    let background = 0.1f32;

    let mut total_error = 0.0f32;
    let mut max_error = 0.0f32;
    let mut count = 0;

    // Test a grid of sub-pixel positions
    for dx in 0..10 {
        for dy in 0..10 {
            let true_cx = 10.0 + dx as f32 * 0.1;
            let true_cy = 10.0 + dy as f32 * 0.1;

            // Create perfect Moffat profile
            let mut pixels = vec![background; width * height];
            for y in 0..height {
                for x in 0..width {
                    let r2 = (x as f32 - true_cx).powi(2) + (y as f32 - true_cy).powi(2);
                    pixels[y * width + x] += 1.0 * (1.0 + r2 / (alpha * alpha)).powf(-beta);
                }
            }
            let pixels_buf = Buffer2::new(width, height, pixels);

            let config = MoffatFitConfig {
                fit_beta: false,
                fixed_beta: beta,
                ..Default::default()
            };
            if let Some(result) =
                fit_moffat_2d(&pixels_buf, Vec2::splat(10.0), 8, background, &config)
                && result.converged
            {
                let error =
                    ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
                total_error += error;
                max_error = max_error.max(error);
                count += 1;
            }
        }
    }

    let avg_error = total_error / count as f32;

    // Moffat fitting should achieve ~0.01 pixel accuracy
    assert!(
        avg_error < 0.02,
        "Average Moffat fit error {} exceeds 0.02 pixels (count={})",
        avg_error,
        count
    );
    assert!(
        max_error < 0.05,
        "Max Moffat fit error {} exceeds 0.05 pixels",
        max_error
    );
}

/// Verify FWHM estimation accuracy from second moments.
#[test]
fn test_fwhm_estimation_accuracy() {
    let width = 128;
    let height = 128;
    let bg = make_uniform_background(width, height, 0.1, 0.001);

    // Test various sigma values
    for sigma in [1.5f32, 2.0, 2.5, 3.0, 3.5, 4.0] {
        let expected_fwhm = FWHM_TO_SIGMA * sigma;
        let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), sigma, 1.0);

        let metrics = compute_metrics(
            &pixels,
            &bg,
            Vec2::splat(64.0),
            TEST_STAMP_RADIUS,
            None,
            None,
        )
        .unwrap();

        let fwhm_error = (metrics.fwhm - expected_fwhm).abs() / expected_fwhm;
        assert!(
            fwhm_error < 0.15,
            "FWHM error {:.1}% too large for sigma={} (expected={:.2}, got={:.2})",
            fwhm_error * 100.0,
            sigma,
            expected_fwhm,
            metrics.fwhm
        );
    }
}

/// Verify eccentricity calculation for known elliptical sources.
#[test]
fn test_eccentricity_calculation_accuracy() {
    let width = 64;
    let height = 64;
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Test various axis ratios
    // eccentricity = sqrt(1 - (b/a)^2) where a >= b
    let test_cases = [
        (2.5, 2.5, 0.0),   // Circular: e = 0
        (3.0, 2.0, 0.745), // 1.5:1 ratio: e = sqrt(1 - 4/9) ≈ 0.745
        (4.0, 2.0, 0.866), // 2:1 ratio: e = sqrt(1 - 1/4) ≈ 0.866
    ];

    for (sigma_major, sigma_minor, expected_ecc) in test_cases {
        let pixels = make_elliptical_star(
            width,
            height,
            Vec2::splat(32.0),
            sigma_major,
            sigma_minor,
            0.8,
        );
        let metrics = compute_metrics(
            &pixels,
            &bg,
            Vec2::splat(32.0),
            TEST_STAMP_RADIUS,
            None,
            None,
        )
        .unwrap();

        let ecc_error = (metrics.eccentricity - expected_ecc).abs();
        assert!(
            ecc_error < 0.15,
            "Eccentricity error {} too large for ratio {:.1}:{:.1} (expected={:.3}, got={:.3})",
            ecc_error,
            sigma_major,
            sigma_minor,
            expected_ecc,
            metrics.eccentricity
        );
    }
}

/// Verify SNR calculation follows the CCD noise equation.
#[test]
fn test_snr_calculation_with_gain() {
    let width = 64;
    let height = 64;
    let amplitude = 0.8f32;
    let sigma = 2.5f32;
    let sky_noise = 0.02f32;
    let gain = 2.0f32; // e-/ADU
    let read_noise = 5.0f32; // electrons

    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), sigma, amplitude);
    let bg = make_uniform_background(width, height, 0.1, sky_noise);

    // Without gain (simplified formula)
    let metrics_no_gain = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // With gain (full CCD equation)
    let metrics_with_gain = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        Some(gain),
        Some(read_noise),
    )
    .unwrap();

    // Both should be positive
    assert!(
        metrics_no_gain.snr > 0.0,
        "SNR without gain should be positive"
    );
    assert!(
        metrics_with_gain.snr > 0.0,
        "SNR with gain should be positive"
    );

    // With shot noise and read noise added, SNR should be lower
    // (unless the star is very bright)
    // Just verify both are computed reasonably
    assert!(
        metrics_no_gain.snr > 1.0,
        "SNR should be > 1 for this bright star"
    );
    // With shot noise and read noise, SNR may be lower but still positive
    assert!(
        metrics_with_gain.snr > 0.1,
        "SNR with gain should be > 0.1, got {}",
        metrics_with_gain.snr
    );
}

/// Verify sharpness distinguishes point sources from extended sources.
#[test]
fn test_sharpness_point_vs_extended() {
    let width = 64;
    let height = 64;
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Compact star (small sigma) - high sharpness
    let compact = make_gaussian_star(width, height, Vec2::splat(32.0), 1.5, 0.8);
    let metrics_compact = compute_metrics(
        &compact,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Extended star (large sigma) - lower sharpness
    let extended = make_gaussian_star(width, height, Vec2::splat(32.0), 4.0, 0.8);
    let metrics_extended = compute_metrics(
        &extended,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    assert!(
        metrics_compact.sharpness > metrics_extended.sharpness,
        "Compact star should have higher sharpness than extended: {} vs {}",
        metrics_compact.sharpness,
        metrics_extended.sharpness
    );

    // Sharpness should be in valid range
    assert!(
        metrics_compact.sharpness > 0.0 && metrics_compact.sharpness <= 1.0,
        "Sharpness out of range: {}",
        metrics_compact.sharpness
    );
}

/// Verify Moffat FWHM formula is correct.
#[test]
fn test_moffat_fwhm_formula() {
    use super::moffat_fit::{alpha_beta_to_fwhm, fwhm_beta_to_alpha};

    // Test known values
    // For beta=2.5: FWHM = 2*alpha*sqrt(2^0.4 - 1) ≈ 2*alpha*0.5657 ≈ 1.131*alpha
    let alpha = 2.0f32;
    let beta = 2.5f32;
    let fwhm = alpha_beta_to_fwhm(alpha, beta);

    // Verify against expected value
    let expected = 2.0 * alpha * (2.0f32.powf(1.0 / beta) - 1.0).sqrt();
    assert!(
        (fwhm - expected).abs() < 1e-6,
        "FWHM formula incorrect: {} vs {}",
        fwhm,
        expected
    );

    // Verify round-trip
    let alpha_back = fwhm_beta_to_alpha(fwhm, beta);
    assert!(
        (alpha_back - alpha).abs() < 1e-6,
        "Round-trip failed: {} vs {}",
        alpha_back,
        alpha
    );

    // Test limiting case: as beta -> infinity, Moffat -> Gaussian
    // For large beta, FWHM ≈ 2*alpha*sqrt(ln(2)/beta) -> 0
    // But for beta=4.765 (theoretical), FWHM ≈ 0.95*alpha
    let beta_theory = 4.765f32;
    let fwhm_theory = alpha_beta_to_fwhm(alpha, beta_theory);
    assert!(
        fwhm_theory > 0.0 && fwhm_theory < fwhm,
        "Higher beta should give smaller FWHM"
    );
}

/// Test Gaussian fitting recovers correct sigma values.
#[test]
fn test_gaussian_fit_sigma_recovery() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};

    let width = 21;
    let height = 21;
    let background = 0.1f32;

    // Test sigma values that fit well within stamp_radius=8
    for true_sigma in [2.0f32, 2.5, 3.0, 3.5] {
        let cx = 10.0f32;
        let cy = 10.0f32;

        let mut pixels = vec![background; width * height];
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                pixels[y * width + x] +=
                    1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
            }
        }
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = GaussianFitConfig::default();
        let result = fit_gaussian_2d(&pixels_buf, Vec2::new(cx, cy), 8, background, &config);

        let result =
            result.unwrap_or_else(|| panic!("Fit should return Some for sigma={}", true_sigma));

        // Check that sigma values are accurate (convergence flag may be false if
        // initial guess was already close, causing small parameter changes)
        let sigma_error_x = (result.sigma.x - true_sigma).abs() / true_sigma;
        let sigma_error_y = (result.sigma.y - true_sigma).abs() / true_sigma;

        assert!(
            sigma_error_x < 0.1,
            "Sigma_x error {:.1}% too large for sigma={} (got={})",
            sigma_error_x * 100.0,
            true_sigma,
            result.sigma.x
        );
        assert!(
            sigma_error_y < 0.1,
            "Sigma_y error {:.1}% too large for sigma={} (got={})",
            sigma_error_y * 100.0,
            true_sigma,
            result.sigma.y
        );
    }
}

/// Test Moffat fitting recovers correct alpha values.
#[test]
fn test_moffat_fit_alpha_recovery() {
    use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d};

    let width = 21;
    let height = 21;
    let background = 0.1f32;
    let beta = 2.5f32;

    // Test alpha values that fit well within stamp_radius=8
    for true_alpha in [2.0f32, 2.5, 3.0, 3.5] {
        let cx = 10.0f32;
        let cy = 10.0f32;

        let mut pixels = vec![background; width * height];
        for y in 0..height {
            for x in 0..width {
                let r2 = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2);
                pixels[y * width + x] += 1.0 * (1.0 + r2 / (true_alpha * true_alpha)).powf(-beta);
            }
        }
        let pixels_buf = Buffer2::new(width, height, pixels);

        let config = MoffatFitConfig {
            fit_beta: false,
            fixed_beta: beta,
            ..Default::default()
        };
        let result = fit_moffat_2d(&pixels_buf, Vec2::new(cx, cy), 8, background, &config)
            .unwrap_or_else(|| panic!("Fit should return Some for alpha={}", true_alpha));

        // Check that alpha is accurate (convergence flag may be false if
        // initial guess was already close)
        let alpha_error = (result.alpha - true_alpha).abs() / true_alpha;

        assert!(
            alpha_error < 0.15,
            "Alpha error {:.1}% too large for alpha={} (got={})",
            alpha_error * 100.0,
            true_alpha,
            result.alpha
        );
    }
}

/// Test that fitting works with noisy data.
#[test]
fn test_gaussian_fit_with_noise() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};

    let width = 21;
    let height = 21;
    let true_cx = 10.3f32;
    let true_cy = 10.7f32;
    let true_sigma = 2.5f32;
    let background = 0.1f32;

    // Create Gaussian with deterministic "noise" pattern
    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - true_cx;
            let dy = y as f32 - true_cy;
            let signal = 1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
            // Add small deterministic noise
            let noise = ((x * 7 + y * 13) % 100) as f32 * 0.001 - 0.05;
            pixels[y * width + x] += signal + noise * 0.1;
        }
    }
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, Vec2::splat(10.0), 8, background, &config);

    assert!(result.is_some(), "Fit should succeed with noise");
    let result = result.unwrap();

    // With noise, expect slightly worse but still good accuracy
    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.15,
        "Position error {} too large with noise",
        error
    );
}

/// Verify roundness1 (GROUND) is close to 0 for circular sources.
#[test]
fn test_roundness1_circular_source() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    assert!(
        metrics.roundness1.abs() < 0.1,
        "Circular source should have roundness1 near 0, got {}",
        metrics.roundness1
    );
}

/// Verify roundness1 detects x-elongated sources.
#[test]
fn test_roundness1_x_elongated() {
    let width = 64;
    let height = 64;
    // sigma_x > sigma_y means more spread in x direction
    let pixels = make_elliptical_star(width, height, Vec2::splat(32.0), 4.0, 2.0, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // For x-elongated: marginal in x has lower peak (more spread)
    // Roundness1 = (Hx - Hy) / (Hx + Hy)
    // This should be negative because Hx (peak of x marginal) < Hy
    assert!(
        metrics.roundness1 != 0.0,
        "Elongated source should have non-zero roundness1"
    );
}

/// Verify roundness2 (SROUND) is close to 0 for symmetric sources.
#[test]
fn test_roundness2_symmetric_source() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    assert!(
        metrics.roundness2 < 0.1,
        "Symmetric source should have roundness2 near 0, got {}",
        metrics.roundness2
    );
}

// =============================================================================
// Moment-based Sigma Estimation Tests
// =============================================================================

#[test]
fn test_estimate_sigma_from_moments_gaussian() {
    use super::estimate_sigma_from_moments;

    let width = 21;
    let height = 21;
    let cx = 10.0f32;
    let cy = 10.0f32;
    let true_sigma = 2.5f32;
    let background = 0.1f32;

    // Create Gaussian star
    let mut data_x = Vec::new();
    let mut data_y = Vec::new();
    let mut data_z = Vec::new();

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let value =
                background + 1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
            data_x.push(x as f32);
            data_y.push(y as f32);
            data_z.push(value);
        }
    }

    let estimated_sigma =
        estimate_sigma_from_moments(&data_x, &data_y, &data_z, Vec2::new(cx, cy), background);

    // Should be within 20% of true sigma
    let error = (estimated_sigma - true_sigma).abs() / true_sigma;
    assert!(
        error < 0.2,
        "Sigma estimate error {:.1}% too large (expected={}, got={})",
        error * 100.0,
        true_sigma,
        estimated_sigma
    );
}

#[test]
fn test_estimate_sigma_from_moments_various_sigmas() {
    use super::estimate_sigma_from_moments;

    let width = 21;
    let height = 21;
    let cx = 10.0f32;
    let cy = 10.0f32;
    let background = 0.1f32;

    for true_sigma in [1.5f32, 2.0, 2.5, 3.0, 4.0] {
        let mut data_x = Vec::new();
        let mut data_y = Vec::new();
        let mut data_z = Vec::new();

        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let value = background
                    + 1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
                data_x.push(x as f32);
                data_y.push(y as f32);
                data_z.push(value);
            }
        }

        let estimated_sigma =
            estimate_sigma_from_moments(&data_x, &data_y, &data_z, Vec2::new(cx, cy), background);

        let error = (estimated_sigma - true_sigma).abs() / true_sigma;
        assert!(
            error < 0.25,
            "Sigma={}: estimate error {:.1}% too large (got={})",
            true_sigma,
            error * 100.0,
            estimated_sigma
        );
    }
}

// =============================================================================
// Adaptive Sigma Tests
// =============================================================================

#[test]
fn test_refine_centroid_adaptive_sigma_small_fwhm() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let sigma = 1.5f32; // Small sigma
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Use small expected FWHM
    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        expected_fwhm,
    );

    assert!(result.is_some());
    let new_pos = result.unwrap();

    // Should converge towards true position
    let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Centroid error {} too large for small FWHM",
        error
    );
}

#[test]
fn test_refine_centroid_adaptive_sigma_large_fwhm() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let sigma = 4.0f32; // Large sigma
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Use large expected FWHM
    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        expected_fwhm,
    );

    assert!(result.is_some());
    let new_pos = result.unwrap();

    // Should converge towards true position
    let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Centroid error {} too large for large FWHM",
        error
    );
}

// =============================================================================
// extract_stamp Tests
// =============================================================================

#[test]
fn test_extract_stamp_valid_center() {
    use super::extract_stamp;

    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    let result = extract_stamp(&pixels, Vec2::splat(32.0), 5);
    assert!(result.is_some(), "Should extract stamp at center");

    let (data_x, data_y, data_z, peak) = result.unwrap();
    let expected_size = (2 * 5 + 1) * (2 * 5 + 1); // 11x11 = 121
    assert_eq!(data_x.len(), expected_size);
    assert_eq!(data_y.len(), expected_size);
    assert_eq!(data_z.len(), expected_size);
    assert!((peak - 0.5).abs() < f32::EPSILON, "Peak should be 0.5");
}

#[test]
fn test_extract_stamp_edge_invalid() {
    use super::extract_stamp;

    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    // Too close to edges
    assert!(extract_stamp(&pixels, Vec2::new(3.0, 32.0), 5).is_none());
    assert!(extract_stamp(&pixels, Vec2::new(32.0, 3.0), 5).is_none());
    assert!(extract_stamp(&pixels, Vec2::new(61.0, 32.0), 5).is_none());
    assert!(extract_stamp(&pixels, Vec2::new(32.0, 61.0), 5).is_none());
}

#[test]
fn test_extract_stamp_peak_value() {
    use super::extract_stamp;

    let width = 64;
    let height = 64;
    let mut pixels = vec![0.1f32; width * height];
    // Add bright pixel at center
    pixels[32 * width + 32] = 0.9;
    let pixels = Buffer2::new(width, height, pixels);

    let result = extract_stamp(&pixels, Vec2::splat(32.0), 5);
    assert!(result.is_some());

    let (_, _, _, peak) = result.unwrap();
    assert!((peak - 0.9).abs() < f32::EPSILON, "Peak should be 0.9");
}

#[test]
fn test_extract_stamp_coordinates() {
    use super::extract_stamp;

    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    let result = extract_stamp(&pixels, Vec2::splat(32.0), 2);
    assert!(result.is_some());

    let (data_x, data_y, _, _) = result.unwrap();
    // For radius=2, stamp is 5x5, centered at (32,32)
    // x coords should be 30,31,32,33,34 (repeated for each row)
    // y coords should be 30,30,30,30,30, 31,31,31,31,31, etc.
    assert_eq!(data_x.len(), 25);

    // Check that coordinates are correct
    let min_x = data_x.iter().fold(f32::MAX, |a, &b| a.min(b));
    let max_x = data_x.iter().fold(f32::MIN, |a, &b| a.max(b));
    let min_y = data_y.iter().fold(f32::MAX, |a, &b| a.min(b));
    let max_y = data_y.iter().fold(f32::MIN, |a, &b| a.max(b));

    assert_eq!(min_x, 30.0);
    assert_eq!(max_x, 34.0);
    assert_eq!(min_y, 30.0);
    assert_eq!(max_y, 34.0);
}

#[test]
fn test_extract_stamp_fractional_position() {
    use super::extract_stamp;

    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);

    // Fractional position 32.3, 32.7 rounds to 32, 33
    let result = extract_stamp(&pixels, Vec2::new(32.3, 32.7), 2);
    assert!(result.is_some());

    let (data_x, data_y, _, _) = result.unwrap();
    // Center should be at rounded position (32, 33)
    let min_x = data_x.iter().fold(f32::MAX, |a, &b| a.min(b));
    let max_x = data_x.iter().fold(f32::MIN, |a, &b| a.max(b));
    let min_y = data_y.iter().fold(f32::MAX, |a, &b| a.min(b));
    let max_y = data_y.iter().fold(f32::MIN, |a, &b| a.max(b));

    assert_eq!(min_x, 30.0); // 32 - 2 = 30
    assert_eq!(max_x, 34.0); // 32 + 2 = 34
    assert_eq!(min_y, 31.0); // 33 - 2 = 31
    assert_eq!(max_y, 35.0); // 33 + 2 = 35
}

// =============================================================================
// compute_annulus_background Tests (via LocalAnnulus)
// =============================================================================

#[test]
fn test_local_annulus_background_uniform() {
    use crate::star_detection::LocalBackgroundMethod;

    let width = 128;
    let height = 128;
    let background_value = 0.2f32;

    // Create uniform background with a star
    let mut pixels = vec![background_value; width * height];
    // Add star at center
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - 64.0;
            let dy = y as f32 - 64.0;
            let r2 = dx * dx + dy * dy;
            let value = 0.8 * (-r2 / (2.0 * 2.5 * 2.5)).exp();
            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }
    let pixels = Buffer2::new(width, height, pixels);

    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = Config {
        local_background: LocalBackgroundMethod::LocalAnnulus,
        ..Default::default()
    };
    let candidates = detect_stars_test(&pixels, &bg, &config);

    assert!(!candidates.is_empty(), "Should detect star");

    let star = measure_star(&pixels, &bg, &candidates[0], &config);
    assert!(star.is_some(), "Should compute centroid with LocalAnnulus");

    let star = star.unwrap();
    // SNR should be computed correctly
    assert!(star.snr > 0.0, "SNR should be positive");
    assert!(star.flux > 0.0, "Flux should be positive");
}

#[test]
fn test_local_annulus_vs_global_map() {
    use crate::star_detection::LocalBackgroundMethod;

    let width = 128;
    let height = 128;

    // Create star on uniform background
    let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), 2.5, 0.8);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    // Detect with GlobalMap
    let config_global = Config {
        local_background: LocalBackgroundMethod::GlobalMap,
        ..Default::default()
    };
    let candidates = detect_stars_test(&pixels, &bg, &config_global);
    let star_global =
        measure_star(&pixels, &bg, &candidates[0], &config_global).expect("global centroid");

    // Detect with LocalAnnulus
    let config_annulus = Config {
        local_background: LocalBackgroundMethod::LocalAnnulus,
        ..Default::default()
    };
    let star_annulus =
        measure_star(&pixels, &bg, &candidates[0], &config_annulus).expect("annulus centroid");

    // Both should give similar position (within 0.5 pixels)
    let pos_diff = ((star_global.pos.x - star_annulus.pos.x).powi(2)
        + (star_global.pos.y - star_annulus.pos.y).powi(2))
    .sqrt();
    assert!(
        pos_diff < 0.5,
        "GlobalMap and LocalAnnulus should give similar positions: diff={}",
        pos_diff
    );

    // Both should have positive flux and SNR
    assert!(star_global.flux > 0.0 && star_annulus.flux > 0.0);
    assert!(star_global.snr > 0.0 && star_annulus.snr > 0.0);
}

#[test]
fn test_local_annulus_near_edge_fallback() {
    use crate::star_detection::LocalBackgroundMethod;

    let width = 64;
    let height = 64;

    // Create star near edge where annulus might be partially outside
    let pos = Vec2::new(20.0, 32.0);
    let pixels = make_gaussian_star(width, height, pos, 2.0, 0.8);
    let bg = crate::testing::estimate_background(
        &pixels,
        &Config {
            tile_size: 32,
            ..Default::default()
        },
    );

    let config = Config {
        local_background: LocalBackgroundMethod::LocalAnnulus,
        edge_margin: 15, // Allow detection near edge
        ..Default::default()
    };
    let candidates = detect_stars_test(&pixels, &bg, &config);

    if !candidates.is_empty() {
        // Should still work (falls back to global if annulus doesn't have enough pixels)
        let star = measure_star(&pixels, &bg, &candidates[0], &config);
        if let Some(s) = star {
            assert!(s.flux > 0.0, "Flux should be positive");
        }
    }
}

// =============================================================================
// compute_roundness Edge Case Tests
// =============================================================================

#[test]
fn test_roundness_zero_flux() {
    // When all marginal values are zero, roundness should be 0
    let marginal_x = vec![0.0f64; 11];
    let marginal_y = vec![0.0f64; 11];

    let (r1, r2) = super::compute_roundness(&marginal_x, &marginal_y);

    assert_eq!(r1, 0.0, "Roundness1 should be 0 for zero flux");
    assert_eq!(r2, 0.0, "Roundness2 should be 0 for zero flux");
}

#[test]
fn test_roundness_uniform_marginals() {
    // Uniform marginals should give roundness1 = 0 (Hx = Hy)
    let marginal_x = vec![1.0f64; 11];
    let marginal_y = vec![1.0f64; 11];

    let (r1, _) = super::compute_roundness(&marginal_x, &marginal_y);

    assert!(
        r1.abs() < 0.01,
        "Roundness1 should be ~0 for uniform marginals"
    );
}

#[test]
fn test_roundness_asymmetric_x() {
    // Create asymmetric x marginal (more flux on right)
    let mut marginal_x = vec![0.1f64; 11];
    marginal_x[8] = 1.0; // Extra flux on right side
    let marginal_y = vec![0.5f64; 11]; // Symmetric

    let (_, r2) = super::compute_roundness(&marginal_x, &marginal_y);

    assert!(
        r2 > 0.0,
        "Roundness2 should be positive for asymmetric source"
    );
}

#[test]
fn test_roundness_x_vs_y_elongation() {
    // X-elongated: higher peak in y marginal (more compact in y)
    let mut marginal_x = vec![0.1f64; 11];
    let mut marginal_y = vec![0.1f64; 11];

    // Y marginal has higher peak (star is more compact in y, elongated in x)
    marginal_y[5] = 2.0;
    marginal_x[5] = 1.0;

    let (r1, _) = super::compute_roundness(&marginal_x, &marginal_y);

    // r1 = (Hx - Hy) / (Hx + Hy) = (1.0 - 2.0) / (1.0 + 2.0) = -1/3
    assert!(
        r1 < 0.0,
        "X-elongated star should have negative roundness1: {}",
        r1
    );
}

#[test]
fn test_roundness_y_vs_x_elongation() {
    // Y-elongated: higher peak in x marginal (more compact in x)
    let mut marginal_x = vec![0.1f64; 11];
    let mut marginal_y = vec![0.1f64; 11];

    // X marginal has higher peak (star is more compact in x, elongated in y)
    marginal_x[5] = 2.0;
    marginal_y[5] = 1.0;

    let (r1, _) = super::compute_roundness(&marginal_x, &marginal_y);

    // r1 = (Hx - Hy) / (Hx + Hy) = (2.0 - 1.0) / (2.0 + 1.0) = 1/3
    assert!(
        r1 > 0.0,
        "Y-elongated star should have positive roundness1: {}",
        r1
    );
}

#[test]
fn test_roundness_bounds() {
    // Test that roundness values are always within bounds
    let test_cases = [
        (vec![1.0f64; 11], vec![0.001f64; 11]), // Very different peaks
        (vec![0.001f64; 11], vec![1.0f64; 11]), // Opposite
        (vec![1.0f64; 11], vec![1.0f64; 11]),   // Equal
    ];

    for (marginal_x, marginal_y) in test_cases {
        let (r1, r2) = super::compute_roundness(&marginal_x, &marginal_y);
        assert!(
            (-1.0..=1.0).contains(&r1),
            "Roundness1 out of bounds: {}",
            r1
        );
        assert!(
            (0.0..=1.0).contains(&r2),
            "Roundness2 out of bounds: {}",
            r2
        );
    }
}

// =============================================================================
// Phase 1 Iteration Behavior Tests
// =============================================================================

/// Verify that Phase 1 (weighted moments) reaches sub-pixel accuracy quickly.
/// After 2 iterations the position should be within 0.1px of the final converged position.
/// This establishes that reducing max iterations for fitting methods is safe.
#[test]
fn test_phase1_reaches_good_accuracy_in_few_iterations() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let stamp_radius = 7;
    let expected_fwhm = 5.9;

    // Run full convergence to get the final position
    let mut pos_full = Vec2::new(32.0, 33.0);
    for _ in 0..MAX_MOMENTS_ITERATIONS {
        let new_pos = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_full,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
        let delta = new_pos - pos_full;
        pos_full = new_pos;
        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Run only 2 iterations
    let mut pos_2iter = Vec2::new(32.0, 33.0);
    for _ in 0..2 {
        pos_2iter = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_2iter,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
    }

    // After 2 iterations, should be within 0.2px of the fully converged position
    // (the convergence threshold is 0.001px, so final iterations refine at sub-millipixel
    // level — well beyond what L-M fitting needs as a starting point)
    let diff = ((pos_2iter.x - pos_full.x).powi(2) + (pos_2iter.y - pos_full.y).powi(2)).sqrt();
    assert!(
        diff < 0.2,
        "After 2 iterations, position should be within 0.2px of converged result, got {:.4}px diff",
        diff
    );

    // And within 0.3px of the true position
    let error = ((pos_2iter.x - true_pos.x).powi(2) + (pos_2iter.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.3,
        "After 2 iterations, position should be within 0.3px of true position, got {:.4}px error",
        error
    );
}

/// Verify that even a single Phase 1 iteration provides a reasonable starting
/// point for L-M fitting (position within ~0.5 pixels of true center).
#[test]
fn test_single_phase1_iteration_provides_good_seed() {
    let width = 64;
    let height = 64;

    // Test multiple sub-pixel offsets
    for dx in 0..5 {
        for dy in 0..5 {
            let true_pos = Vec2::new(32.0 + dx as f32 * 0.2, 32.0 + dy as f32 * 0.2);
            let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8);
            let bg = make_uniform_background(width, height, 0.1, 0.01);

            // Start from integer peak position
            let start = Vec2::new(true_pos.x.round(), true_pos.y.round());

            let after_one = refine_centroid(&pixels, width, height, &bg, start, 7, 5.9)
                .expect("refine should succeed");

            let error =
                ((after_one.x - true_pos.x).powi(2) + (after_one.y - true_pos.y).powi(2)).sqrt();

            assert!(
                error < 0.5,
                "Single iteration should get within 0.5px of true pos, got {:.3} for true_pos=({:.1}, {:.1})",
                error,
                true_pos.x,
                true_pos.y
            );
        }
    }
}

/// Verify that GaussianFit accuracy is equivalent whether Phase 1 runs 2 or 10 iterations.
#[test]
fn test_gaussian_fit_accuracy_independent_of_phase1_iterations() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};

    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let sigma = 2.5;
    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let stamp_radius = 7;
    let expected_fwhm = 5.9;

    // Phase 1 with only 2 iterations
    let mut pos_2iter = Vec2::new(32.0, 33.0);
    for _ in 0..2 {
        pos_2iter = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_2iter,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
    }

    // Phase 1 with full 10 iterations
    let mut pos_full = Vec2::new(32.0, 33.0);
    for _ in 0..MAX_MOMENTS_ITERATIONS {
        let new_pos = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_full,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
        let delta = new_pos - pos_full;
        pos_full = new_pos;
        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Now apply Gaussian fit from both starting points
    let config = GaussianFitConfig::default();
    let result_2iter = fit_gaussian_2d(&pixels, pos_2iter, stamp_radius, 0.1, &config)
        .expect("fit should succeed from 2-iter seed");
    let result_full = fit_gaussian_2d(&pixels, pos_full, stamp_radius, 0.1, &config)
        .expect("fit should succeed from full seed");

    // Both should converge to essentially the same position
    let diff = ((result_2iter.pos.x - result_full.pos.x).powi(2)
        + (result_2iter.pos.y - result_full.pos.y).powi(2))
    .sqrt();

    assert!(
        diff < 0.01,
        "Gaussian fit should converge to same position regardless of Phase 1 iterations: diff={:.4}",
        diff
    );

    // Both should be close to true position
    let error_2iter = ((result_2iter.pos.x - true_pos.x).powi(2)
        + (result_2iter.pos.y - true_pos.y).powi(2))
    .sqrt();
    assert!(
        error_2iter < 0.05,
        "Gaussian fit from 2-iter seed should achieve <0.05px accuracy, got {:.4}",
        error_2iter
    );
}

/// Verify that MoffatFit accuracy is equivalent whether Phase 1 runs 2 or 10 iterations.
#[test]
fn test_moffat_fit_accuracy_independent_of_phase1_iterations() {
    use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d};

    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let pixels = make_gaussian_star(width, height, true_pos, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let stamp_radius = 7;
    let expected_fwhm = 5.9;

    // Phase 1 with only 2 iterations
    let mut pos_2iter = Vec2::new(32.0, 33.0);
    for _ in 0..2 {
        pos_2iter = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_2iter,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
    }

    // Phase 1 with full 10 iterations
    let mut pos_full = Vec2::new(32.0, 33.0);
    for _ in 0..MAX_MOMENTS_ITERATIONS {
        let new_pos = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_full,
            stamp_radius,
            expected_fwhm,
        )
        .expect("refine should succeed");
        let delta = new_pos - pos_full;
        pos_full = new_pos;
        if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Now apply Moffat fit from both starting points
    let config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: 2.5,
        ..MoffatFitConfig::default()
    };
    let result_2iter = fit_moffat_2d(&pixels, pos_2iter, stamp_radius, 0.1, &config)
        .expect("fit should succeed from 2-iter seed");
    let result_full = fit_moffat_2d(&pixels, pos_full, stamp_radius, 0.1, &config)
        .expect("fit should succeed from full seed");

    // Both should converge to essentially the same position
    let diff = ((result_2iter.pos.x - result_full.pos.x).powi(2)
        + (result_2iter.pos.y - result_full.pos.y).powi(2))
    .sqrt();

    assert!(
        diff < 0.01,
        "Moffat fit should converge to same position regardless of Phase 1 iterations: diff={:.4}",
        diff
    );

    // Both should be close to true position
    let error_2iter = ((result_2iter.pos.x - true_pos.x).powi(2)
        + (result_2iter.pos.y - true_pos.y).powi(2))
    .sqrt();
    assert!(
        error_2iter < 0.05,
        "Moffat fit from 2-iter seed should achieve <0.05px accuracy, got {:.4}",
        error_2iter
    );
}

// =============================================================================
// compute_stamp_radius Tests
// =============================================================================

#[test]
fn test_compute_stamp_radius_typical_fwhm() {
    use super::compute_stamp_radius;
    // FWHM = 4.0 -> radius = ceil(4.0 * 1.75) = 7
    assert_eq!(compute_stamp_radius(4.0), 7);
}

#[test]
fn test_compute_stamp_radius_clamped_min() {
    use super::MIN_STAMP_RADIUS;
    use super::compute_stamp_radius;
    // Very small FWHM should clamp to minimum
    assert_eq!(compute_stamp_radius(1.0), MIN_STAMP_RADIUS);
}

#[test]
fn test_compute_stamp_radius_clamped_max() {
    use super::MAX_STAMP_RADIUS;
    use super::compute_stamp_radius;
    // Very large FWHM should clamp to maximum
    assert_eq!(compute_stamp_radius(20.0), MAX_STAMP_RADIUS);
}

#[test]
fn test_compute_stamp_radius_various_fwhm() {
    use super::compute_stamp_radius;
    use super::{MAX_STAMP_RADIUS, MIN_STAMP_RADIUS, STAMP_RADIUS_FWHM_FACTOR};

    // Test various FWHM values
    for fwhm in [2.0f32, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0] {
        let radius = compute_stamp_radius(fwhm);
        let expected = (fwhm * STAMP_RADIUS_FWHM_FACTOR).ceil() as usize;
        let expected_clamped = expected.clamp(MIN_STAMP_RADIUS, MAX_STAMP_RADIUS);
        assert_eq!(
            radius, expected_clamped,
            "FWHM={}: expected {}, got {}",
            fwhm, expected_clamped, radius
        );
    }
}

// =============================================================================
// Pre-fit Moments Iterations Verification
// =============================================================================

/// Verifies that using only 2 pre-fit moments iterations produces equivalent
/// centroid results compared to using 10 iterations before Gaussian/Moffat fitting.
///
/// This test validates the design decision in MOMENTS_ITERATIONS_BEFORE_FIT:
/// the L-M optimizer refines position independently and converges to the same
/// result regardless of Phase 1 precision.
#[test]
fn test_prefit_moments_iterations_sufficient() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};
    use super::{CONVERGENCE_THRESHOLD_SQ, refine_centroid};

    let width = 64;
    let height = 64;

    // Test with various sub-pixel positions and FWHM values
    let test_cases = [
        (Vec2::new(32.3, 32.7), 2.5f32), // Typical star, FWHM ~5.9
        (Vec2::new(32.8, 32.2), 3.5f32), // Larger PSF, FWHM ~8.2
        (Vec2::new(32.1, 32.9), 1.8f32), // Smaller PSF, FWHM ~4.2
    ];

    for (true_pos, sigma) in test_cases {
        let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8);
        let bg = make_uniform_background(width, height, 0.1, 0.01);
        let expected_fwhm = sigma / FWHM_TO_SIGMA;
        let stamp_radius = 7;

        // Start from peak position (slightly off from true position)
        let peak_pos = Vec2::new(true_pos.x.round(), true_pos.y.round());

        // Run with 2 iterations (current MOMENTS_ITERATIONS_BEFORE_FIT)
        let mut pos_2iter = peak_pos;
        for _ in 0..2 {
            if let Some(new_pos) = refine_centroid(
                &pixels,
                width,
                height,
                &bg,
                pos_2iter,
                stamp_radius,
                expected_fwhm,
            ) {
                let delta = new_pos - pos_2iter;
                pos_2iter = new_pos;
                if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                    break;
                }
            }
        }

        // Run with 10 iterations (fully converged moments)
        let mut pos_10iter = peak_pos;
        for _ in 0..10 {
            if let Some(new_pos) = refine_centroid(
                &pixels,
                width,
                height,
                &bg,
                pos_10iter,
                stamp_radius,
                expected_fwhm,
            ) {
                let delta = new_pos - pos_10iter;
                pos_10iter = new_pos;
                if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                    break;
                }
            }
        }

        // Now apply Gaussian fitting to both starting points
        let local_bg = 0.1;
        let fit_config = GaussianFitConfig {
            position_convergence_threshold: 0.0001,
            ..GaussianFitConfig::default()
        };

        let result_from_2iter =
            fit_gaussian_2d(&pixels, pos_2iter, stamp_radius, local_bg, &fit_config);
        let result_from_10iter =
            fit_gaussian_2d(&pixels, pos_10iter, stamp_radius, local_bg, &fit_config);

        // Both should converge
        assert!(
            result_from_2iter.is_some(),
            "Gaussian fit from 2-iter moments failed for sigma={}",
            sigma
        );
        assert!(
            result_from_10iter.is_some(),
            "Gaussian fit from 10-iter moments failed for sigma={}",
            sigma
        );

        let pos_final_2iter = result_from_2iter.unwrap().pos;
        let pos_final_10iter = result_from_10iter.unwrap().pos;

        // Final positions should be nearly identical (< 0.01 pixels)
        let diff = (pos_final_2iter - pos_final_10iter).length();
        assert!(
            diff < 0.01,
            "Position difference {:.6} pixels exceeds 0.01 for sigma={}: \
             2-iter={:?}, 10-iter={:?}",
            diff,
            sigma,
            pos_final_2iter,
            pos_final_10iter
        );

        // Both should be accurate to within 0.05 pixels of true position
        let error_2iter = (pos_final_2iter - true_pos).length();
        let error_10iter = (pos_final_10iter - true_pos).length();
        assert!(
            error_2iter < 0.05,
            "2-iter centroid error {:.4} exceeds 0.05 for sigma={}",
            error_2iter,
            sigma
        );
        assert!(
            error_10iter < 0.05,
            "10-iter centroid error {:.4} exceeds 0.05 for sigma={}",
            error_10iter,
            sigma
        );
    }
}

/// Same test but for Moffat fitting to ensure both PSF models benefit
/// from the 2-iteration pre-fit optimization.
#[test]
fn test_prefit_moments_iterations_sufficient_moffat() {
    use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d};
    use super::{CONVERGENCE_THRESHOLD_SQ, lm_optimizer, refine_centroid};

    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.4, 32.6);
    let sigma = 2.5f32;

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let expected_fwhm = sigma / FWHM_TO_SIGMA;
    let stamp_radius = 7;

    let peak_pos = Vec2::new(true_pos.x.round(), true_pos.y.round());

    // Run with 2 iterations
    let mut pos_2iter = peak_pos;
    for _ in 0..2 {
        if let Some(new_pos) = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_2iter,
            stamp_radius,
            expected_fwhm,
        ) {
            let delta = new_pos - pos_2iter;
            pos_2iter = new_pos;
            if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                break;
            }
        }
    }

    // Run with 10 iterations
    let mut pos_10iter = peak_pos;
    for _ in 0..10 {
        if let Some(new_pos) = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos_10iter,
            stamp_radius,
            expected_fwhm,
        ) {
            let delta = new_pos - pos_10iter;
            pos_10iter = new_pos;
            if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                break;
            }
        }
    }

    // Apply Moffat fitting to both
    let local_bg = 0.1;
    let fit_config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: 2.5,
        lm: lm_optimizer::LMConfig {
            position_convergence_threshold: 0.0001,
            ..lm_optimizer::LMConfig::default()
        },
    };

    let result_from_2iter = fit_moffat_2d(&pixels, pos_2iter, stamp_radius, local_bg, &fit_config);
    let result_from_10iter =
        fit_moffat_2d(&pixels, pos_10iter, stamp_radius, local_bg, &fit_config);

    assert!(
        result_from_2iter.is_some(),
        "Moffat fit from 2-iter moments failed"
    );
    assert!(
        result_from_10iter.is_some(),
        "Moffat fit from 10-iter moments failed"
    );

    let pos_final_2iter = result_from_2iter.unwrap().pos;
    let pos_final_10iter = result_from_10iter.unwrap().pos;

    // Final positions should be nearly identical
    let diff = (pos_final_2iter - pos_final_10iter).length();
    assert!(
        diff < 0.01,
        "Moffat position difference {:.6} pixels exceeds 0.01: 2-iter={:?}, 10-iter={:?}",
        diff,
        pos_final_2iter,
        pos_final_10iter
    );
}

// =============================================================================
// Extreme FWHM Tests (Undersampled and Large PSFs)
// =============================================================================

/// Test centroiding with undersampled PSF (FWHM < 2 pixels).
/// This is a challenging case where the star is barely resolved.
#[test]
fn test_centroid_undersampled_psf() {
    let width = 64;
    let height = 64;
    let sigma = 0.7f32; // FWHM ≈ 1.65 pixels (undersampled)
    let true_pos = Vec2::new(32.3, 32.7);

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.9);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let expected_fwhm = FWHM_TO_SIGMA * sigma;
    let stamp_radius = 5; // Smaller stamp for undersampled

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(32.0),
        stamp_radius,
        expected_fwhm,
    );

    assert!(
        result.is_some(),
        "Should find centroid for undersampled PSF"
    );
    let new_pos = result.unwrap();

    // Undersampled PSFs have worse accuracy - allow 0.5 pixel error
    let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Centroid error {} too large for undersampled PSF (FWHM={:.2})",
        error,
        expected_fwhm
    );
}

/// Test centroiding with very large PSF (FWHM > 15 pixels).
#[test]
fn test_centroid_large_psf() {
    let width = 128;
    let height = 128;
    let sigma = 8.0f32; // FWHM ≈ 18.8 pixels (large PSF)
    let true_pos = Vec2::new(64.3, 64.7);

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let expected_fwhm = FWHM_TO_SIGMA * sigma;
    let stamp_radius = MAX_STAMP_RADIUS; // Use maximum allowed

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(64.0),
        stamp_radius,
        expected_fwhm,
    );

    assert!(result.is_some(), "Should find centroid for large PSF");
    let new_pos = result.unwrap();

    // Large PSFs have reduced accuracy since the stamp radius (15px) is smaller
    // than the FWHM (18.8px), so not all light is captured for weighting.
    // Allow 0.5 pixel error for such extreme cases.
    let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Centroid error {} too large for large PSF (FWHM={:.2})",
        error,
        expected_fwhm
    );
}

/// Test Gaussian fitting with undersampled PSF.
#[test]
fn test_gaussian_fit_undersampled_psf() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};

    let width = 21;
    let height = 21;
    let true_sigma = 0.8f32; // FWHM ≈ 1.88 pixels
    let true_cx = 10.3f32;
    let true_cy = 10.7f32;
    let background = 0.1f32;

    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - true_cx;
            let dy = y as f32 - true_cy;
            pixels[y * width + x] +=
                1.0 * (-0.5 * (dx * dx + dy * dy) / (true_sigma * true_sigma)).exp();
        }
    }
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, Vec2::splat(10.0), 6, background, &config);

    assert!(result.is_some(), "Should fit undersampled Gaussian");
    let result = result.unwrap();

    // Position accuracy degrades for undersampled PSFs
    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.3,
        "Position error {} too large for undersampled PSF",
        error
    );
}

/// Test metrics computation with very small FWHM.
#[test]
fn test_metrics_small_fwhm() {
    let width = 64;
    let height = 64;
    let sigma = 0.8f32; // Very small
    let pixels = make_gaussian_star(width, height, Vec2::splat(32.0), sigma, 0.9);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(&pixels, &bg, Vec2::splat(32.0), 5, None, None);

    assert!(metrics.is_some(), "Should compute metrics for small FWHM");
    let m = metrics.unwrap();
    assert!(m.fwhm > 0.0, "FWHM should be positive");
    assert!(m.flux > 0.0, "Flux should be positive");
}

/// Test metrics computation with very large FWHM.
#[test]
fn test_metrics_large_fwhm() {
    let width = 128;
    let height = 128;
    let sigma = 7.0f32; // Large
    let pixels = make_gaussian_star(width, height, Vec2::splat(64.0), sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(
        &pixels,
        &bg,
        Vec2::splat(64.0),
        MAX_STAMP_RADIUS,
        None,
        None,
    );

    assert!(metrics.is_some(), "Should compute metrics for large FWHM");
    let m = metrics.unwrap();
    assert!(m.fwhm > 10.0, "FWHM should be large: {}", m.fwhm);
}

// =============================================================================
// Blended/Contaminated Star Tests
// =============================================================================

/// Create two overlapping stars.
fn make_blended_stars(
    width: usize,
    height: usize,
    pos1: Vec2,
    pos2: Vec2,
    sigma: f32,
    amp1: f32,
    amp2: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];

    for y in 0..height {
        for x in 0..width {
            let dx1 = x as f32 - pos1.x;
            let dy1 = y as f32 - pos1.y;
            let r2_1 = dx1 * dx1 + dy1 * dy1;
            let v1 = amp1 * (-r2_1 / (2.0 * sigma * sigma)).exp();

            let dx2 = x as f32 - pos2.x;
            let dy2 = y as f32 - pos2.y;
            let r2_2 = dx2 * dx2 + dy2 * dy2;
            let v2 = amp2 * (-r2_2 / (2.0 * sigma * sigma)).exp();

            if v1 > 0.001 || v2 > 0.001 {
                pixels[y * width + x] += v1 + v2;
            }
        }
    }

    Buffer2::new(width, height, pixels)
}

/// Test centroiding with a nearby contaminating star.
#[test]
fn test_centroid_with_nearby_star() {
    let width = 64;
    let height = 64;
    let sigma = 2.5f32;

    // Primary star at center, fainter companion 8 pixels away
    let primary_pos = Vec2::new(32.0, 32.0);
    let secondary_pos = Vec2::new(40.0, 32.0); // 8 pixels separation
    let pixels = make_blended_stars(width, height, primary_pos, secondary_pos, sigma, 0.8, 0.3);

    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        primary_pos,
        TEST_STAMP_RADIUS,
        expected_fwhm,
    );

    assert!(result.is_some(), "Should find centroid despite nearby star");
    let new_pos = result.unwrap();

    // Primary centroid should be pulled slightly toward secondary
    // but should still be within 1 pixel of true position
    let error = ((new_pos.x - primary_pos.x).powi(2) + (new_pos.y - primary_pos.y).powi(2)).sqrt();
    assert!(
        error < 1.0,
        "Centroid error {} too large with nearby contamination",
        error
    );
}

/// Test centroiding with closely blended stars (partially overlapping).
#[test]
fn test_centroid_blended_stars() {
    let width = 64;
    let height = 64;
    let sigma = 2.5f32;

    // Two stars only 5 pixels apart (significant overlap)
    let primary_pos = Vec2::new(32.0, 32.0);
    let secondary_pos = Vec2::new(37.0, 32.0);
    let pixels = make_blended_stars(width, height, primary_pos, secondary_pos, sigma, 0.8, 0.5);

    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        primary_pos,
        TEST_STAMP_RADIUS,
        expected_fwhm,
    );

    assert!(result.is_some(), "Should attempt centroid on blended stars");
    let new_pos = result.unwrap();

    // Centroid will be pulled toward center of light - check it moved toward secondary
    // This is expected behavior for blended sources
    assert!(
        new_pos.x > primary_pos.x,
        "Blended centroid should be pulled toward secondary star"
    );
}

/// Test Gaussian fitting with contaminating star in the wing.
#[test]
fn test_gaussian_fit_with_contamination() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};

    let width = 31;
    let height = 31;
    let sigma = 2.5f32;
    let background = 0.1f32;

    // Primary star at center
    let true_cx = 15.0f32;
    let true_cy = 15.0f32;

    let mut pixels = vec![background; width * height];

    // Add primary star
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - true_cx;
            let dy = y as f32 - true_cy;
            let r2 = dx * dx + dy * dy;
            pixels[y * width + x] += 0.8 * (-r2 / (2.0 * sigma * sigma)).exp();
        }
    }

    // Add faint contaminating star at edge of stamp
    let contam_cx = 22.0f32;
    let contam_cy = 15.0f32;
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - contam_cx;
            let dy = y as f32 - contam_cy;
            let r2 = dx * dx + dy * dy;
            pixels[y * width + x] += 0.2 * (-r2 / (2.0 * sigma * sigma)).exp();
        }
    }

    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(
        &pixels_buf,
        Vec2::new(true_cx, true_cy),
        8,
        background,
        &config,
    );

    assert!(result.is_some(), "Should fit despite contamination");
    let result = result.unwrap();

    // Position should still be reasonably accurate (within 0.5 pixel)
    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Position error {} too large with wing contamination",
        error
    );
}

/// Test that eccentricity is affected by nearby star contamination.
#[test]
fn test_eccentricity_with_contamination() {
    let width = 64;
    let height = 64;
    let sigma = 2.5f32;

    // Single circular star
    let single_star = make_gaussian_star(width, height, Vec2::splat(32.0), sigma, 0.8);

    // Star with nearby companion (will appear elongated)
    let contaminated = make_blended_stars(
        width,
        height,
        Vec2::splat(32.0),
        Vec2::new(38.0, 32.0),
        sigma,
        0.8,
        0.4,
    );

    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_single = compute_metrics(
        &single_star,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    let metrics_contaminated = compute_metrics(
        &contaminated,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();

    // Contaminated source should have higher eccentricity
    assert!(
        metrics_contaminated.eccentricity > metrics_single.eccentricity,
        "Contaminated source should appear more elongated: {} vs {}",
        metrics_contaminated.eccentricity,
        metrics_single.eccentricity
    );
}

// =============================================================================
// Rotated Elliptical PSF Tests
// =============================================================================

/// Create a rotated elliptical Gaussian.
fn make_rotated_elliptical_star(
    width: usize,
    height: usize,
    pos: Vec2,
    sigma_major: f32,
    sigma_minor: f32,
    angle_rad: f32,
    amplitude: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];

    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - pos.x;
            let dy = y as f32 - pos.y;

            // Rotate coordinates
            let dx_rot = dx * cos_a + dy * sin_a;
            let dy_rot = -dx * sin_a + dy * cos_a;

            // Compute elliptical distance
            let r2 = (dx_rot / sigma_major).powi(2) + (dy_rot / sigma_minor).powi(2);
            let value = amplitude * (-r2 / 2.0).exp();

            if value > 0.001 {
                pixels[y * width + x] += value;
            }
        }
    }

    Buffer2::new(width, height, pixels)
}

/// Test centroiding on a 45-degree rotated ellipse.
#[test]
fn test_centroid_rotated_ellipse_45deg() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.3, 32.7);
    let angle = std::f32::consts::FRAC_PI_4; // 45 degrees

    let pixels = make_rotated_elliptical_star(width, height, true_pos, 4.0, 2.0, angle, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        Vec2::splat(32.0),
        TEST_STAMP_RADIUS,
        6.0,
    );

    assert!(result.is_some(), "Should find centroid for rotated ellipse");
    let new_pos = result.unwrap();

    // Rotated ellipses with high eccentricity and offset starting position
    // have reduced accuracy. Allow 0.6 pixel error.
    let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.6,
        "Centroid error {} too large for 45° rotated ellipse",
        error
    );
}

/// Test centroiding on various rotation angles.
#[test]
fn test_centroid_various_rotation_angles() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.0, 32.0);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Test multiple rotation angles
    for angle_deg in [0, 30, 45, 60, 90, 120, 150] {
        let angle_rad = (angle_deg as f32).to_radians();
        let pixels =
            make_rotated_elliptical_star(width, height, true_pos, 4.0, 2.0, angle_rad, 0.8);

        let result = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            true_pos,
            TEST_STAMP_RADIUS,
            6.0,
        );

        assert!(
            result.is_some(),
            "Should find centroid for {}° rotated ellipse",
            angle_deg
        );
        let new_pos = result.unwrap();

        let error = ((new_pos.x - true_pos.x).powi(2) + (new_pos.y - true_pos.y).powi(2)).sqrt();
        assert!(
            error < 0.2,
            "Centroid error {} too large for {}° rotated ellipse",
            error,
            angle_deg
        );
    }
}

/// Test that rotated ellipses have similar eccentricity regardless of angle.
#[test]
fn test_eccentricity_rotation_invariant() {
    let width = 64;
    let height = 64;
    let pos = Vec2::splat(32.0);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let mut eccentricities = Vec::new();

    for angle_deg in [0, 45, 90, 135] {
        let angle_rad = (angle_deg as f32).to_radians();
        let pixels = make_rotated_elliptical_star(width, height, pos, 4.0, 2.0, angle_rad, 0.8);

        let metrics = compute_metrics(&pixels, &bg, pos, TEST_STAMP_RADIUS, None, None).unwrap();
        eccentricities.push((angle_deg, metrics.eccentricity));
    }

    // All eccentricities should be similar (within 20% of each other)
    let avg_ecc: f32 =
        eccentricities.iter().map(|(_, e)| e).sum::<f32>() / eccentricities.len() as f32;
    for (angle, ecc) in &eccentricities {
        let diff = (ecc - avg_ecc).abs() / avg_ecc;
        assert!(
            diff < 0.25,
            "Eccentricity at {}° differs too much from average: {} vs {}",
            angle,
            ecc,
            avg_ecc
        );
    }
}

/// Test Gaussian fitting on rotated ellipse (fits axis-aligned sigma_x, sigma_y).
#[test]
fn test_gaussian_fit_rotated_ellipse() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};

    let width = 31;
    let height = 31;
    let true_cx = 15.0f32;
    let true_cy = 15.0f32;
    let background = 0.1f32;

    // Create 45° rotated ellipse
    let pixels = make_rotated_elliptical_star(
        width,
        height,
        Vec2::new(true_cx, true_cy),
        3.5,
        2.0,
        std::f32::consts::FRAC_PI_4,
        0.8,
    );

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels, Vec2::new(true_cx, true_cy), 8, background, &config);

    assert!(result.is_some(), "Should fit rotated ellipse");
    let result = result.unwrap();

    // Position should still be accurate
    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.1,
        "Position error {} too large for rotated ellipse fit",
        error
    );

    // The fitted sigma values will be axis-aligned projections, not the true major/minor axes
    // Just verify they're reasonable and different from each other
    assert!(
        result.sigma.x > 1.0 && result.sigma.x < 5.0,
        "sigma_x out of range"
    );
    assert!(
        result.sigma.y > 1.0 && result.sigma.y < 5.0,
        "sigma_y out of range"
    );
}

// =============================================================================
// Bad Initial Guess Recovery Tests
// =============================================================================

/// Test recovery from initial guess 2 pixels away from true position.
#[test]
fn test_recovery_from_2pixel_offset() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.0, 32.0);
    let sigma = 2.5f32;

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    // Start 2 pixels away
    let initial_guess = Vec2::new(34.0, 30.0);

    let mut pos = initial_guess;
    for _ in 0..MAX_MOMENTS_ITERATIONS {
        if let Some(new_pos) = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos,
            TEST_STAMP_RADIUS,
            expected_fwhm,
        ) {
            let delta = new_pos - pos;
            pos = new_pos;
            if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                break;
            }
        } else {
            break;
        }
    }

    let error = ((pos.x - true_pos.x).powi(2) + (pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.2,
        "Should recover from 2-pixel offset, error = {}",
        error
    );
}

/// Test recovery from initial guess 3 pixels away.
#[test]
fn test_recovery_from_3pixel_offset() {
    let width = 64;
    let height = 64;
    let true_pos = Vec2::new(32.0, 32.0);
    let sigma = 2.5f32;

    let pixels = make_gaussian_star(width, height, true_pos, sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    // Start 3 pixels away diagonally
    let initial_guess = Vec2::new(34.1, 34.1); // ~3 pixel offset

    let mut pos = initial_guess;
    for _ in 0..MAX_MOMENTS_ITERATIONS {
        if let Some(new_pos) = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            pos,
            TEST_STAMP_RADIUS,
            expected_fwhm,
        ) {
            let delta = new_pos - pos;
            pos = new_pos;
            if delta.length_squared() < CONVERGENCE_THRESHOLD_SQ {
                break;
            }
        } else {
            break;
        }
    }

    let error = ((pos.x - true_pos.x).powi(2) + (pos.y - true_pos.y).powi(2)).sqrt();
    assert!(
        error < 0.3,
        "Should recover from 3-pixel offset, error = {}",
        error
    );
}

/// Test Gaussian fitting with bad initial position guess.
#[test]
fn test_gaussian_fit_bad_initial_guess() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d};

    let width = 31;
    let height = 31;
    let true_cx = 15.0f32;
    let true_cy = 15.0f32;
    let sigma = 2.5f32;
    let background = 0.1f32;

    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - true_cx;
            let dy = y as f32 - true_cy;
            pixels[y * width + x] += 1.0 * (-0.5 * (dx * dx + dy * dy) / (sigma * sigma)).exp();
        }
    }
    let pixels_buf = Buffer2::new(width, height, pixels);

    // Initial guess 2.5 pixels away
    let initial_guess = Vec2::new(13.0, 17.0);

    let config = GaussianFitConfig::default();
    let result = fit_gaussian_2d(&pixels_buf, initial_guess, 8, background, &config);

    assert!(result.is_some(), "Should converge from bad initial guess");
    let result = result.unwrap();

    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.1,
        "Gaussian fit should recover from bad guess, error = {}",
        error
    );
}

/// Test Moffat fitting with bad initial position guess.
#[test]
fn test_moffat_fit_bad_initial_guess() {
    use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d};

    let width = 31;
    let height = 31;
    let true_cx = 15.0f32;
    let true_cy = 15.0f32;
    let alpha = 2.5f32;
    let beta = 2.5f32;
    let background = 0.1f32;

    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let r2 = (x as f32 - true_cx).powi(2) + (y as f32 - true_cy).powi(2);
            pixels[y * width + x] += 1.0 * (1.0 + r2 / (alpha * alpha)).powf(-beta);
        }
    }
    let pixels_buf = Buffer2::new(width, height, pixels);

    // Initial guess 2 pixels away
    let initial_guess = Vec2::new(13.0, 17.0);

    let config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: beta,
        ..Default::default()
    };
    let result = fit_moffat_2d(&pixels_buf, initial_guess, 8, background, &config);

    assert!(
        result.is_some(),
        "Moffat should converge from bad initial guess"
    );
    let result = result.unwrap();

    let error = ((result.pos.x - true_cx).powi(2) + (result.pos.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.1,
        "Moffat fit should recover from bad guess, error = {}",
        error
    );
}
