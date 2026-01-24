//! Tests for centroid computation.

use super::*;
use crate::star_detection::background::estimate_background;
use crate::star_detection::detection::detect_stars;

fn make_gaussian_star(
    width: usize,
    height: usize,
    cx: f32,
    cy: f32,
    sigma: f32,
    amplitude: f32,
) -> Vec<f32> {
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

    pixels
}

#[test]
fn test_centroid_accuracy() {
    // Use larger image to minimize background estimation effects
    let width = 128;
    let height = 128;
    let true_x = 64.3f32;
    let true_y = 64.7f32;
    let pixels = make_gaussian_star(width, height, true_x, true_y, 2.5, 0.8);

    let bg = estimate_background(&pixels, width, height, 32);
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, width, height, &bg, &config);

    assert_eq!(candidates.len(), 1);

    let star = compute_centroid(&pixels, width, height, &bg, &candidates[0], &config)
        .expect("Should compute centroid");

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
    let expected_fwhm = 2.355 * sigma;
    let pixels = make_gaussian_star(width, height, 64.0, 64.0, sigma, 0.8);

    let bg = estimate_background(&pixels, width, height, 32);
    // Use higher max_area because dilation (radius=2) expands the star region
    let config = StarDetectionConfig {
        max_area: 1000,
        ..StarDetectionConfig::default()
    };
    let candidates = detect_stars(&pixels, width, height, &bg, &config);

    assert_eq!(candidates.len(), 1);

    let star = compute_centroid(&pixels, width, height, &bg, &candidates[0], &config)
        .expect("Should compute centroid");

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

    let bg = estimate_background(&pixels, width, height, 32);
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, width, height, &bg, &config);

    let star = compute_centroid(&pixels, width, height, &bg, &candidates[0], &config)
        .expect("Should compute centroid");

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

    let bg = estimate_background(&pixels, width, height, 32);
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, width, height, &bg, &config);

    let star = compute_centroid(&pixels, width, height, &bg, &candidates[0], &config)
        .expect("Should compute centroid");

    assert!(star.snr > 0.0, "SNR should be positive");
    assert!(star.flux > 0.0, "Flux should be positive");
}
