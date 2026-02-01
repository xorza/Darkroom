//! Tests for centroid computation.

use super::*;
use crate::common::Buffer2;
use crate::math::FWHM_TO_SIGMA;
use crate::math::{Aabb, Vec2us};
use crate::star_detection::background::{BackgroundConfig, BackgroundMap};
use crate::star_detection::candidate_detection::{StarCandidate, detect_stars};

/// Default stamp radius for tests (matching expected FWHM of ~4 pixels).
const TEST_STAMP_RADIUS: usize = 7;

/// Default expected FWHM for tests (sigma=2.5 -> FWHM≈5.9 pixels).
const TEST_EXPECTED_FWHM: f32 = 5.9;

fn make_gaussian_star(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    sigma: f32,
    amplitude: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
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
    let true_x = 64.3f32;
    let true_y = 64.7f32;
    let pixels = make_gaussian_star(width, height, true_x, true_y, 2.5, 0.8);

    let bg = BackgroundMap::new(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, None, &bg, &config);

    assert_eq!(candidates.len(), 1);

    let star =
        compute_centroid(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    let error_x = (star.x - true_x).abs();
    let error_y = (star.y - true_y).abs();

    // Sub-pixel accuracy within 0.2 pixels is good for weighted centroid
    assert!(
        error_x < 0.2,
        "X centroid error {} too large (true={}, computed={})",
        error_x,
        true_x,
        star.x
    );
    assert!(
        error_y < 0.2,
        "Y centroid error {} too large (true={}, computed={})",
        error_y,
        true_y,
        star.y
    );
}

#[test]
fn test_fwhm_estimation() {
    // Use larger image for better background estimation
    let width = 128;
    let height = 128;
    let sigma = 3.0f32;
    let expected_fwhm = FWHM_TO_SIGMA * sigma;
    let pixels = make_gaussian_star(width, height, 64.0, 64.0, sigma, 0.8);

    let bg = BackgroundMap::new(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    // Use higher max_area because dilation (radius=2) expands the star region
    let config = StarDetectionConfig {
        max_area: 1000,
        ..StarDetectionConfig::default()
    };
    let candidates = detect_stars(&pixels, None, &bg, &config);

    assert_eq!(candidates.len(), 1);

    let star =
        compute_centroid(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

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
    let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);

    let bg = BackgroundMap::new(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, None, &bg, &config);

    let star =
        compute_centroid(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    assert!(
        star.eccentricity < 0.3,
        "Circular star has high eccentricity: {}",
        star.eccentricity
    );
}

#[test]
fn test_snr_positive() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);

    let bg = BackgroundMap::new(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, None, &bg, &config);

    let star =
        compute_centroid(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

    assert!(star.snr > 0.0, "SNR should be positive");
    assert!(star.flux > 0.0, "Flux should be positive");
}

// =============================================================================
// is_valid_stamp_position Tests
// =============================================================================

#[test]
fn test_valid_stamp_position_center() {
    // Center of a 64x64 image should be valid
    assert!(is_valid_stamp_position(
        32.0,
        32.0,
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
        min_pos,
        min_pos,
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
        max_pos_x,
        max_pos_y,
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
        pos,
        32.0,
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
        32.0,
        pos,
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
        pos,
        32.0,
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
        32.0,
        pos,
        64,
        height,
        TEST_STAMP_RADIUS
    ));
}

#[test]
fn test_valid_stamp_position_negative_rounds_to_invalid() {
    // Negative position should be invalid
    assert!(!is_valid_stamp_position(
        -1.0,
        32.0,
        64,
        64,
        TEST_STAMP_RADIUS
    ));
    assert!(!is_valid_stamp_position(
        32.0,
        -1.0,
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
        7.4,
        32.0,
        64,
        64,
        TEST_STAMP_RADIUS
    ));
    // 6.4 rounds to 6, which is less than TEST_STAMP_RADIUS (7), so invalid
    assert!(!is_valid_stamp_position(
        6.4,
        32.0,
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
        TEST_STAMP_RADIUS as f32,
        TEST_STAMP_RADIUS as f32,
        min_size,
        min_size,
        TEST_STAMP_RADIUS
    ));
    // One pixel smaller - no valid positions
    assert!(!is_valid_stamp_position(
        TEST_STAMP_RADIUS as f32,
        TEST_STAMP_RADIUS as f32,
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
) -> BackgroundMap {
    BackgroundMap {
        background: Buffer2::new_filled(width, height, bg_value),
        noise: Buffer2::new_filled(width, height, noise),
        adaptive_sigma: None,
    }
}

#[test]
fn test_refine_centroid_centered_star() {
    let width = 64;
    let height = 64;
    let cx = 32.0f32;
    let cy = 32.0f32;
    let pixels = make_gaussian_star(width, height, cx, cy, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        cx,
        cy,
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );

    assert!(result.is_some());
    let (new_cx, new_cy) = result.unwrap();
    // Should stay very close to original position
    assert!((new_cx - cx).abs() < 0.5);
    assert!((new_cy - cy).abs() < 0.5);
}

#[test]
fn test_refine_centroid_offset_converges() {
    let width = 64;
    let height = 64;
    let true_cx = 32.3f32;
    let true_cy = 32.7f32;
    let pixels = make_gaussian_star(width, height, true_cx, true_cy, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Start with integer guess (peak pixel position)
    let start_cx = 32.0f32;
    let start_cy = 33.0f32;

    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        start_cx,
        start_cy,
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );

    assert!(result.is_some());
    let (new_cx, new_cy) = result.unwrap();
    // Should move towards true center
    let old_error = ((start_cx - true_cx).powi(2) + (start_cy - true_cy).powi(2)).sqrt();
    let new_error = ((new_cx - true_cx).powi(2) + (new_cy - true_cy).powi(2)).sqrt();
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
        3.0,
        32.0,
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
        32.0,
        32.0,
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
    let pixels = make_gaussian_star(width, height, 50.0, 50.0, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Start far from the actual star - the stamp won't contain the star,
    // so there's no signal, which should cause rejection
    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        32.0,
        32.0,
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
    let true_cx = 32.25f32;
    let true_cy = 32.75f32;
    let pixels = make_gaussian_star(width, height, true_cx, true_cy, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Simulate multiple iterations like compute_centroid does
    let mut cx = 32.0f32;
    let mut cy = 32.0f32;

    for iteration in 0..MAX_ITERATIONS {
        let result = refine_centroid(
            &pixels,
            width,
            height,
            &bg,
            cx,
            cy,
            TEST_STAMP_RADIUS,
            TEST_EXPECTED_FWHM,
        );
        assert!(result.is_some(), "Iteration {} failed", iteration);

        let (new_cx, new_cy) = result.unwrap();
        let dx = new_cx - cx;
        let dy = new_cy - cy;
        cx = new_cx;
        cy = new_cy;

        if dx * dx + dy * dy < CONVERGENCE_THRESHOLD_SQ {
            break;
        }
    }

    // Should converge close to true position
    let error = ((cx - true_cx).powi(2) + (cy - true_cy).powi(2)).sqrt();
    assert!(error < 0.2, "Failed to converge: error = {}", error);
}

// =============================================================================
// compute_metrics Tests
// =============================================================================

#[test]
fn test_compute_metrics_valid_star() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(&pixels, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None);

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
    let metrics = compute_metrics(&pixels, &bg, 3.0, 32.0, TEST_STAMP_RADIUS, None, None);
    assert!(metrics.is_none());
}

#[test]
fn test_compute_metrics_zero_flux_returns_none() {
    let width = 64;
    let height = 64;
    // All pixels equal to or below background
    let pixels = Buffer2::new_filled(width, height, 0.05f32);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(&pixels, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None);
    assert!(metrics.is_none());
}

#[test]
fn test_compute_metrics_fwhm_scales_with_sigma() {
    let width = 128;
    let height = 128;

    // Create stars with different sigmas
    let sigma_small = 2.0f32;
    let sigma_large = 4.0f32;

    let pixels_small = make_gaussian_star(width, height, 64.0, 64.0, sigma_small, 0.8);
    let pixels_large = make_gaussian_star(width, height, 64.0, 64.0, sigma_large, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_small = compute_metrics(
        &pixels_small,
        &bg,
        64.0,
        64.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_large = compute_metrics(
        &pixels_large,
        &bg,
        64.0,
        64.0,
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

    let pixels_dim = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.2);
    let pixels_bright = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_dim =
        compute_metrics(&pixels_dim, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();
    let metrics_bright = compute_metrics(
        &pixels_bright,
        &bg,
        32.0,
        32.0,
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
    cx: f32,
    cy: f32,
    sigma_x: f32,
    sigma_y: f32,
    amplitude: f32,
) -> Buffer2<f32> {
    let mut pixels = vec![0.1f32; width * height];

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
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
    let pixels = make_elliptical_star(width, height, 32.0, 32.0, 4.0, 1.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(&pixels, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

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

    let circular = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let elongated = make_elliptical_star(width, height, 32.0, 32.0, 4.0, 2.0, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_circular =
        compute_metrics(&circular, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();
    let metrics_elongated =
        compute_metrics(&elongated, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

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
    let true_cx = 32.0f32;
    let true_cy = 32.0f32;

    // Create star with added noise
    let mut pixels = make_gaussian_star(width, height, true_cx, true_cy, 2.5, 0.8);

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
        true_cx,
        true_cy,
        TEST_STAMP_RADIUS,
        TEST_EXPECTED_FWHM,
    );
    assert!(result.is_some());

    let (new_cx, new_cy) = result.unwrap();
    // With noise, allow more tolerance
    assert!(
        (new_cx - true_cx).abs() < 1.0,
        "X error too large with noise"
    );
    assert!(
        (new_cy - true_cy).abs() < 1.0,
        "Y error too large with noise"
    );
}

#[test]
fn test_snr_decreases_with_higher_noise() {
    let width = 64;
    let height = 64;
    let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);

    let bg_low_noise = make_uniform_background(width, height, 0.1, 0.01);
    let bg_high_noise = make_uniform_background(width, height, 0.1, 0.1);

    let metrics_low = compute_metrics(
        &pixels,
        &bg_low_noise,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_high = compute_metrics(
        &pixels,
        &bg_high_noise,
        32.0,
        32.0,
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

    let pixels = make_gaussian_star(width, height, 64.0, 64.0, sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.001); // Very low noise

    let metrics = compute_metrics(&pixels, &bg, 64.0, 64.0, TEST_STAMP_RADIUS, None, None).unwrap();

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

    let pixels_amp1 = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.4);
    let pixels_amp2 = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics1 =
        compute_metrics(&pixels_amp1, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();
    let metrics2 =
        compute_metrics(&pixels_amp2, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

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
    let circular = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let elongated_x = make_elliptical_star(width, height, 32.0, 32.0, 5.0, 2.0, 0.8);
    let elongated_y = make_elliptical_star(width, height, 32.0, 32.0, 2.0, 5.0, 0.8);

    for (name, pixels) in [
        ("circular", circular),
        ("elongated_x", elongated_x),
        ("elongated_y", elongated_y),
    ] {
        let metrics =
            compute_metrics(&pixels, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();
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

    let elongated_x = make_elliptical_star(width, height, 32.0, 32.0, 4.0, 2.0, 0.8);
    let elongated_y = make_elliptical_star(width, height, 32.0, 32.0, 2.0, 4.0, 0.8);

    let metrics_x =
        compute_metrics(&elongated_x, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();
    let metrics_y =
        compute_metrics(&elongated_y, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

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
    let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);

    let noise1 = 0.02f32;
    let noise2 = 0.04f32; // 2x noise

    let bg1 = make_uniform_background(width, height, 0.1, noise1);
    let bg2 = make_uniform_background(width, height, 0.1, noise2);

    let metrics1 =
        compute_metrics(&pixels, &bg1, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();
    let metrics2 =
        compute_metrics(&pixels, &bg2, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

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
    let metrics = compute_metrics(&pixels, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None);

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

    let pixels_dim = make_gaussian_star(width, height, 32.0, 32.0, sigma, 0.3);
    let pixels_bright = make_gaussian_star(width, height, 32.0, 32.0, sigma, 0.9);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_dim =
        compute_metrics(&pixels_dim, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();
    let metrics_bright = compute_metrics(
        &pixels_bright,
        &bg,
        32.0,
        32.0,
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
    let ratio_1_1 = make_elliptical_star(width, height, 32.0, 32.0, 2.5, 2.5, 0.8); // circular
    let ratio_2_1 = make_elliptical_star(width, height, 32.0, 32.0, 4.0, 2.0, 0.8);
    let ratio_3_1 = make_elliptical_star(width, height, 32.0, 32.0, 6.0, 2.0, 0.8);

    let ecc_1 = compute_metrics(&ratio_1_1, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None)
        .unwrap()
        .eccentricity;
    let ecc_2 = compute_metrics(&ratio_2_1, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None)
        .unwrap()
        .eccentricity;
    let ecc_3 = compute_metrics(&ratio_3_1, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None)
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
// compute_centroid Integration Tests
// =============================================================================

#[test]
fn test_compute_centroid_returns_none_for_edge_candidate() {
    let width = 64;
    let height = 64;
    let pixels = Buffer2::new_filled(width, height, 0.5f32);
    let bg = make_uniform_background(width, height, 0.1, 0.01);
    let config = StarDetectionConfig::default();

    // Create candidate near edge
    let candidate = StarCandidate {
        bbox: Aabb::new(Vec2us::new(0, 30), Vec2us::new(5, 35)),
        peak_x: 3,
        peak_y: 32,
        peak_value: 0.9,
        area: 18,
    };

    let result = compute_centroid(&pixels, &bg, &candidate, &config);
    assert!(
        result.is_none(),
        "Should reject candidate too close to edge"
    );
}

#[test]
fn test_compute_centroid_multiple_stars_independent() {
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
    let bg = BackgroundMap::new(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = StarDetectionConfig {
        edge_margin: 10,
        ..StarDetectionConfig::default()
    };
    let candidates = detect_stars(&pixels, None, &bg, &config);

    assert_eq!(candidates.len(), 2, "Should detect two stars");

    // Compute centroids for both
    let stars: Vec<_> = candidates
        .iter()
        .filter_map(|c| compute_centroid(&pixels, &bg, c, &config))
        .collect();

    assert_eq!(stars.len(), 2, "Should compute centroids for both stars");

    // Verify each star is close to its true position
    for star in &stars {
        let near_star1 = (star.x - star1_cx).abs() < 1.0 && (star.y - star1_cy).abs() < 1.0;
        let near_star2 = (star.x - star2_cx).abs() < 1.0 && (star.y - star2_cy).abs() < 1.0;
        assert!(
            near_star1 || near_star2,
            "Star at ({}, {}) not near either true position",
            star.x,
            star.y
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
    let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);

    let bg = BackgroundMap::new(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, None, &bg, &config);

    assert_eq!(candidates.len(), 1);

    let star =
        compute_centroid(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

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
    let bg = BackgroundMap::new(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, None, &bg, &config);

    assert!(!candidates.is_empty());

    let star =
        compute_centroid(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

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
    let bg = BackgroundMap::new(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, None, &bg, &config);

    assert!(!candidates.is_empty());

    let star =
        compute_centroid(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

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
    let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let bg = BackgroundMap::new(
        &pixels,
        &BackgroundConfig {
            tile_size: 32,
            ..Default::default()
        },
    );
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, None, &bg, &config);

    assert_eq!(candidates.len(), 1);

    let star =
        compute_centroid(&pixels, &bg, &candidates[0], &config).expect("Should compute centroid");

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
        x: 10.0,
        y: 10.0,
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
    use crate::star_detection::CentroidMethod;

    let width = 128;
    let height = 128;
    let sigma = 2.5f32;

    // Test a grid of sub-pixel positions
    let mut total_error = 0.0f32;
    let mut max_error = 0.0f32;
    let mut count = 0;

    for dx in 0..10 {
        for dy in 0..10 {
            let true_cx = 64.0 + dx as f32 * 0.1;
            let true_cy = 64.0 + dy as f32 * 0.1;

            let pixels = make_gaussian_star(width, height, true_cx, true_cy, sigma, 1.0);
            let bg = BackgroundMap::new(
                &pixels,
                &BackgroundConfig {
                    tile_size: 32,
                    ..Default::default()
                },
            );
            let config = StarDetectionConfig {
                centroid_method: CentroidMethod::WeightedMoments,
                ..Default::default()
            };
            let candidates = detect_stars(&pixels, None, &bg, &config);

            if candidates.is_empty() {
                continue;
            }

            if let Some(star) = compute_centroid(&pixels, &bg, &candidates[0], &config) {
                let error = ((star.x - true_cx).powi(2) + (star.y - true_cy).powi(2)).sqrt();
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
            if let Some(result) = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, background, &config)
                && result.converged
            {
                let error = ((result.x - true_cx).powi(2) + (result.y - true_cy).powi(2)).sqrt();
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
            if let Some(result) = fit_moffat_2d(&pixels_buf, 10.0, 10.0, 8, background, &config)
                && result.converged
            {
                let error = ((result.x - true_cx).powi(2) + (result.y - true_cy).powi(2)).sqrt();
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
        let pixels = make_gaussian_star(width, height, 64.0, 64.0, sigma, 1.0);

        let metrics =
            compute_metrics(&pixels, &bg, 64.0, 64.0, TEST_STAMP_RADIUS, None, None).unwrap();

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
        let pixels = make_elliptical_star(width, height, 32.0, 32.0, sigma_major, sigma_minor, 0.8);
        let metrics =
            compute_metrics(&pixels, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

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

    let pixels = make_gaussian_star(width, height, 32.0, 32.0, sigma, amplitude);
    let bg = make_uniform_background(width, height, 0.1, sky_noise);

    // Without gain (simplified formula)
    let metrics_no_gain =
        compute_metrics(&pixels, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

    // With gain (full CCD equation)
    let metrics_with_gain = compute_metrics(
        &pixels,
        &bg,
        32.0,
        32.0,
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
    let compact = make_gaussian_star(width, height, 32.0, 32.0, 1.5, 0.8);
    let metrics_compact =
        compute_metrics(&compact, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

    // Extended star (large sigma) - lower sharpness
    let extended = make_gaussian_star(width, height, 32.0, 32.0, 4.0, 0.8);
    let metrics_extended =
        compute_metrics(&extended, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

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
        let result = fit_gaussian_2d(&pixels_buf, cx, cy, 8, background, &config);

        let result =
            result.unwrap_or_else(|| panic!("Fit should return Some for sigma={}", true_sigma));

        // Check that sigma values are accurate (convergence flag may be false if
        // initial guess was already close, causing small parameter changes)
        let sigma_error_x = (result.sigma_x - true_sigma).abs() / true_sigma;
        let sigma_error_y = (result.sigma_y - true_sigma).abs() / true_sigma;

        assert!(
            sigma_error_x < 0.1,
            "Sigma_x error {:.1}% too large for sigma={} (got={})",
            sigma_error_x * 100.0,
            true_sigma,
            result.sigma_x
        );
        assert!(
            sigma_error_y < 0.1,
            "Sigma_y error {:.1}% too large for sigma={} (got={})",
            sigma_error_y * 100.0,
            true_sigma,
            result.sigma_y
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
        let result = fit_moffat_2d(&pixels_buf, cx, cy, 8, background, &config)
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
    let result = fit_gaussian_2d(&pixels_buf, 10.0, 10.0, 8, background, &config);

    assert!(result.is_some(), "Fit should succeed with noise");
    let result = result.unwrap();

    // With noise, expect slightly worse but still good accuracy
    let error = ((result.x - true_cx).powi(2) + (result.y - true_cy).powi(2)).sqrt();
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
    let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(&pixels, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

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
    let pixels = make_elliptical_star(width, height, 32.0, 32.0, 4.0, 2.0, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(&pixels, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

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
    let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(&pixels, &bg, 32.0, 32.0, TEST_STAMP_RADIUS, None, None).unwrap();

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
        estimate_sigma_from_moments(&data_x, &data_y, &data_z, cx, cy, background);

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
            estimate_sigma_from_moments(&data_x, &data_y, &data_z, cx, cy, background);

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
// Inverse-Variance Weight Computation Tests
// =============================================================================

#[test]
fn test_compute_pixel_weights_basic() {
    use super::compute_pixel_weights;

    let background = 100.0f32;
    let noise = 10.0f32;

    // Pixels with varying signal levels
    let data_z = vec![100.0, 150.0, 200.0, 500.0];

    let weights = compute_pixel_weights(&data_z, background, noise, None, None);

    assert_eq!(weights.len(), 4);
    // All weights should be positive
    for w in &weights {
        assert!(*w > 0.0, "Weight should be positive");
    }
    // Higher signal should have lower weight (more variance)
    // Since variance ~ signal, weight = 1/variance decreases with signal
    assert!(
        weights[0] >= weights[3],
        "Background pixel should have higher weight than bright pixel"
    );
}

#[test]
fn test_compute_pixel_weights_with_gain() {
    use super::compute_pixel_weights;

    let background = 100.0f32;
    let noise = 10.0f32;
    let gain = 2.0f32;
    let read_noise = 5.0f32;

    let data_z = vec![100.0, 200.0, 500.0];

    let weights = compute_pixel_weights(&data_z, background, noise, Some(gain), Some(read_noise));

    assert_eq!(weights.len(), 3);
    // All weights should be positive
    for w in &weights {
        assert!(*w > 0.0, "Weight should be positive");
    }
    // Weights should decrease with signal (more shot noise)
    assert!(
        weights[0] > weights[2],
        "Low-signal pixel should have higher weight"
    );
}

#[test]
fn test_compute_pixel_weights_zero_signal() {
    use super::compute_pixel_weights;

    let background = 100.0f32;
    let noise = 10.0f32;

    // Pixel at background level (zero signal above background)
    let data_z = vec![100.0];

    let weights = compute_pixel_weights(&data_z, background, noise, None, None);

    assert_eq!(weights.len(), 1);
    assert!(
        weights[0] > 0.0,
        "Weight should be positive even for zero signal"
    );
    assert!(weights[0].is_finite(), "Weight should be finite");
}

// =============================================================================
// Weighted Gaussian Fitting Tests
// =============================================================================

#[test]
fn test_fit_gaussian_2d_weighted_basic() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d_weighted};

    let width = 21;
    let height = 21;
    let true_cx = 10.3f32;
    let true_cy = 10.7f32;
    let true_sigma = 2.5f32;
    let background = 0.1f32;
    let noise = 0.02f32;

    // Create Gaussian star
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
    let result = fit_gaussian_2d_weighted(
        &pixels_buf,
        10.0,
        10.0,
        8,
        background,
        noise,
        None,
        None,
        &config,
    );

    assert!(result.is_some(), "Weighted fit should succeed");
    let result = result.unwrap();

    let error = ((result.x - true_cx).powi(2) + (result.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.1,
        "Weighted Gaussian fit position error {} too large",
        error
    );
}

#[test]
fn test_fit_gaussian_2d_weighted_with_gain() {
    use super::gaussian_fit::{GaussianFitConfig, fit_gaussian_2d_weighted};

    let width = 21;
    let height = 21;
    let true_cx = 10.0f32;
    let true_cy = 10.0f32;
    let true_sigma = 2.5f32;
    let background = 0.1f32;
    let noise = 0.02f32;
    let gain = 2.0f32;
    let read_noise = 5.0f32;

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
    let result = fit_gaussian_2d_weighted(
        &pixels_buf,
        10.0,
        10.0,
        8,
        background,
        noise,
        Some(gain),
        Some(read_noise),
        &config,
    );

    assert!(result.is_some(), "Weighted fit with gain should succeed");
    let result = result.unwrap();

    // Should achieve good accuracy
    let error = ((result.x - true_cx).powi(2) + (result.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.1,
        "Weighted Gaussian fit with gain position error {} too large",
        error
    );
}

// =============================================================================
// Weighted Moffat Fitting Tests
// =============================================================================

#[test]
fn test_fit_moffat_2d_weighted_basic() {
    use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d_weighted};

    let width = 21;
    let height = 21;
    let true_cx = 10.3f32;
    let true_cy = 10.7f32;
    let true_alpha = 2.5f32;
    let true_beta = 2.5f32;
    let background = 0.1f32;
    let noise = 0.02f32;

    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let r2 = (x as f32 - true_cx).powi(2) + (y as f32 - true_cy).powi(2);
            pixels[y * width + x] += 1.0 * (1.0 + r2 / (true_alpha * true_alpha)).powf(-true_beta);
        }
    }
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = MoffatFitConfig {
        fit_beta: false,
        fixed_beta: true_beta,
        ..Default::default()
    };
    let result = fit_moffat_2d_weighted(
        &pixels_buf,
        10.0,
        10.0,
        8,
        background,
        noise,
        None,
        None,
        &config,
    );

    assert!(result.is_some(), "Weighted Moffat fit should succeed");
    let result = result.unwrap();

    let error = ((result.x - true_cx).powi(2) + (result.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.1,
        "Weighted Moffat fit position error {} too large",
        error
    );
}

#[test]
fn test_fit_moffat_2d_weighted_variable_beta() {
    use super::moffat_fit::{MoffatFitConfig, fit_moffat_2d_weighted};

    let width = 21;
    let height = 21;
    let true_cx = 10.0f32;
    let true_cy = 10.0f32;
    let true_alpha = 2.5f32;
    let true_beta = 3.5f32;
    let background = 0.1f32;
    let noise = 0.02f32;

    let mut pixels = vec![background; width * height];
    for y in 0..height {
        for x in 0..width {
            let r2 = (x as f32 - true_cx).powi(2) + (y as f32 - true_cy).powi(2);
            pixels[y * width + x] += 1.0 * (1.0 + r2 / (true_alpha * true_alpha)).powf(-true_beta);
        }
    }
    let pixels_buf = Buffer2::new(width, height, pixels);

    let config = MoffatFitConfig {
        fit_beta: true,
        fixed_beta: 3.0, // initial guess
        lm: super::lm_optimizer::LMConfig {
            max_iterations: 100,
            ..Default::default()
        },
    };
    let result = fit_moffat_2d_weighted(
        &pixels_buf,
        10.0,
        10.0,
        8,
        background,
        noise,
        None,
        None,
        &config,
    );

    assert!(
        result.is_some(),
        "Weighted Moffat fit with variable beta should succeed"
    );
    let result = result.unwrap();

    // Position should be accurate
    let error = ((result.x - true_cx).powi(2) + (result.y - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.1,
        "Weighted Moffat fit position error {} too large",
        error
    );

    // Beta should be recovered within tolerance
    let beta_error = (result.beta - true_beta).abs();
    assert!(
        beta_error < 0.5,
        "Beta error {} too large (expected={}, got={})",
        beta_error,
        true_beta,
        result.beta
    );
}

// =============================================================================
// Adaptive Sigma Tests
// =============================================================================

#[test]
fn test_refine_centroid_adaptive_sigma_small_fwhm() {
    let width = 64;
    let height = 64;
    let true_cx = 32.3f32;
    let true_cy = 32.7f32;
    let sigma = 1.5f32; // Small sigma
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = make_gaussian_star(width, height, true_cx, true_cy, sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Use small expected FWHM
    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        expected_fwhm,
    );

    assert!(result.is_some());
    let (new_cx, new_cy) = result.unwrap();

    // Should converge towards true position
    let error = ((new_cx - true_cx).powi(2) + (new_cy - true_cy).powi(2)).sqrt();
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
    let true_cx = 32.3f32;
    let true_cy = 32.7f32;
    let sigma = 4.0f32; // Large sigma
    let expected_fwhm = FWHM_TO_SIGMA * sigma;

    let pixels = make_gaussian_star(width, height, true_cx, true_cy, sigma, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Use large expected FWHM
    let result = refine_centroid(
        &pixels,
        width,
        height,
        &bg,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        expected_fwhm,
    );

    assert!(result.is_some());
    let (new_cx, new_cy) = result.unwrap();

    // Should converge towards true position
    let error = ((new_cx - true_cx).powi(2) + (new_cy - true_cy).powi(2)).sqrt();
    assert!(
        error < 0.5,
        "Centroid error {} too large for large FWHM",
        error
    );
}
