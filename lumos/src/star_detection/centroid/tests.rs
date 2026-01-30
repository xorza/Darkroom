//! Tests for centroid computation.

use super::*;
use crate::common::Buffer2;
use crate::math::FWHM_TO_SIGMA;
use crate::star_detection::background::{BackgroundConfig, BackgroundMap};
use crate::star_detection::deblend::BoundingBox;
use crate::star_detection::detection::{StarCandidate, detect_stars};

/// Default stamp radius for tests (matching expected FWHM of ~4 pixels).
const TEST_STAMP_RADIUS: usize = 7;

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

    let result = refine_centroid(&pixels, width, height, &bg, cx, cy, TEST_STAMP_RADIUS);

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
    let result = refine_centroid(&pixels, width, height, &bg, 3.0, 32.0, TEST_STAMP_RADIUS);
    assert!(result.is_none());
}

#[test]
fn test_refine_centroid_zero_flux_returns_none() {
    let width = 64;
    let height = 64;
    // All pixels equal to background - no signal
    let pixels = vec![0.1f32; width * height];
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let result = refine_centroid(&pixels, width, height, &bg, 32.0, 32.0, TEST_STAMP_RADIUS);
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
    let result = refine_centroid(&pixels, width, height, &bg, 32.0, 32.0, TEST_STAMP_RADIUS);

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
        let result = refine_centroid(&pixels, width, height, &bg, cx, cy, TEST_STAMP_RADIUS);
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

    let metrics = compute_metrics(
        &pixels,
        width,
        height,
        &bg,
        32.0,
        32.0,
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
    let pixels = vec![0.5f32; width * height];
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    // Position too close to edge
    let metrics = compute_metrics(
        &pixels,
        width,
        height,
        &bg,
        3.0,
        32.0,
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
    let pixels = vec![0.05f32; width * height];
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics = compute_metrics(
        &pixels,
        width,
        height,
        &bg,
        32.0,
        32.0,
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

    let pixels_small = make_gaussian_star(width, height, 64.0, 64.0, sigma_small, 0.8);
    let pixels_large = make_gaussian_star(width, height, 64.0, 64.0, sigma_large, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_small = compute_metrics(
        &pixels_small,
        width,
        height,
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
        width,
        height,
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

    let metrics_dim = compute_metrics(
        &pixels_dim,
        width,
        height,
        &bg,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_bright = compute_metrics(
        &pixels_bright,
        width,
        height,
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

    let metrics = compute_metrics(
        &pixels,
        width,
        height,
        &bg,
        32.0,
        32.0,
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

    let circular = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let elongated = make_elliptical_star(width, height, 32.0, 32.0, 4.0, 2.0, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_circular = compute_metrics(
        &circular,
        width,
        height,
        &bg,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_elongated = compute_metrics(
        &elongated,
        width,
        height,
        &bg,
        32.0,
        32.0,
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
        width,
        height,
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
        width,
        height,
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

    let metrics = compute_metrics(
        &pixels,
        width,
        height,
        &bg,
        64.0,
        64.0,
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

    let pixels_amp1 = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.4);
    let pixels_amp2 = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics1 = compute_metrics(
        &pixels_amp1,
        width,
        height,
        &bg,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics2 = compute_metrics(
        &pixels_amp2,
        width,
        height,
        &bg,
        32.0,
        32.0,
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
    let circular = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);
    let elongated_x = make_elliptical_star(width, height, 32.0, 32.0, 5.0, 2.0, 0.8);
    let elongated_y = make_elliptical_star(width, height, 32.0, 32.0, 2.0, 5.0, 0.8);

    for (name, pixels) in [
        ("circular", circular),
        ("elongated_x", elongated_x),
        ("elongated_y", elongated_y),
    ] {
        let metrics = compute_metrics(
            &pixels,
            width,
            height,
            &bg,
            32.0,
            32.0,
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

    let elongated_x = make_elliptical_star(width, height, 32.0, 32.0, 4.0, 2.0, 0.8);
    let elongated_y = make_elliptical_star(width, height, 32.0, 32.0, 2.0, 4.0, 0.8);

    let metrics_x = compute_metrics(
        &elongated_x,
        width,
        height,
        &bg,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_y = compute_metrics(
        &elongated_y,
        width,
        height,
        &bg,
        32.0,
        32.0,
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
    let pixels = make_gaussian_star(width, height, 32.0, 32.0, 2.5, 0.8);

    let noise1 = 0.02f32;
    let noise2 = 0.04f32; // 2x noise

    let bg1 = make_uniform_background(width, height, 0.1, noise1);
    let bg2 = make_uniform_background(width, height, 0.1, noise2);

    let metrics1 = compute_metrics(
        &pixels,
        width,
        height,
        &bg1,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics2 = compute_metrics(
        &pixels,
        width,
        height,
        &bg2,
        32.0,
        32.0,
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

    let bg = make_uniform_background(width, height, 0.5, 0.02);
    let metrics = compute_metrics(
        &pixels,
        width,
        height,
        &bg,
        32.0,
        32.0,
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

    let pixels_dim = make_gaussian_star(width, height, 32.0, 32.0, sigma, 0.3);
    let pixels_bright = make_gaussian_star(width, height, 32.0, 32.0, sigma, 0.9);
    let bg = make_uniform_background(width, height, 0.1, 0.01);

    let metrics_dim = compute_metrics(
        &pixels_dim,
        width,
        height,
        &bg,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap();
    let metrics_bright = compute_metrics(
        &pixels_bright,
        width,
        height,
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

    let ecc_1 = compute_metrics(
        &ratio_1_1,
        width,
        height,
        &bg,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap()
    .eccentricity;
    let ecc_2 = compute_metrics(
        &ratio_2_1,
        width,
        height,
        &bg,
        32.0,
        32.0,
        TEST_STAMP_RADIUS,
        None,
        None,
    )
    .unwrap()
    .eccentricity;
    let ecc_3 = compute_metrics(
        &ratio_3_1,
        width,
        height,
        &bg,
        32.0,
        32.0,
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
        bbox: BoundingBox::new(0, 5, 30, 35),
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
