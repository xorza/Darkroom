//! End-to-end registration tests using synthetic star field images.
//!
//! These tests generate actual pixel images with synthetic stars,
//! run star detection on both images, and verify that registration
//! correctly recovers the applied transformation.

use crate::{AstroImage, ImageDimensions};
use glam::DVec2;

use crate::common::Buffer2;
use crate::registration::config::InterpolationMethod;
use crate::registration::interpolation::{WarpParams, warp_image};
use crate::registration::transform::{Transform, WarpTransform};
use crate::registration::{Config, TransformType, register};
use crate::star_detection::{StarDetector, config::Config as DetConfig};
use crate::testing::synthetic::{self, StarFieldConfig};

/// Default star detector for synthetic images.
fn detector() -> StarDetector {
    StarDetector::from_config(DetConfig {
        expected_fwhm: 0.0, // Disable matched filter for synthetic images
        min_snr: 5.0,
        sigma_threshold: 3.0,
        ..Default::default()
    })
}

/// Apply a similarity transform to an image.
/// Creates a target where stars are visually shifted/rotated/scaled by the given parameters.
/// Passes the inverse to warp_image since it uses output→input coordinate mapping.
fn transform_image(
    src_pixels: &[f32],
    width: usize,
    height: usize,
    dx: f64,
    dy: f64,
    angle_rad: f64,
    scale: f64,
) -> Vec<f32> {
    let transform = Transform::similarity(DVec2::new(dx, dy), angle_rad, scale);
    let inverse = transform.inverse();
    let src_buf = Buffer2::new(width, height, src_pixels.to_vec());
    let mut output = Buffer2::new(width, height, vec![0.0; width * height]);
    warp_image(
        &src_buf,
        &mut output,
        &WarpTransform::new(inverse),
        &WarpParams::new(InterpolationMethod::Bilinear),
    );
    output.into_vec()
}

/// Apply a translation to an image.
/// Creates a target where stars are visually shifted by (dx, dy).
/// Passes the inverse to warp_image since it uses output→input coordinate mapping.
fn translate_image(src_pixels: &[f32], width: usize, height: usize, dx: f64, dy: f64) -> Vec<f32> {
    let transform = Transform::translation(DVec2::new(dx, dy));
    let inverse = transform.inverse();
    let src_buf = Buffer2::new(width, height, src_pixels.to_vec());
    let mut output = Buffer2::new(width, height, vec![0.0; width * height]);
    warp_image(
        &src_buf,
        &mut output,
        &WarpTransform::new(inverse),
        &WarpParams::new(InterpolationMethod::Bilinear),
    );
    output.into_vec()
}

#[test]
fn test_image_registration_translation() {
    // Generate a reference star field image using standard config
    let config = StarFieldConfig {
        num_stars: 50,
        seed: 42,
        ..synthetic::sparse_field_config()
    };
    let width = config.width;
    let height = config.height;

    let (ref_pixels, _ground_truth) = synthetic::generate_star_field(&config);
    let ref_pixels_vec = ref_pixels.into_vec();

    // Apply a known translation to create target image
    let dx = 15.5;
    let dy = -12.3;
    let target_pixels = translate_image(&ref_pixels_vec, width, height, dx, dy);

    // Create AstroImages
    let ref_image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), target_pixels);

    // Detect stars in both images
    let mut det = detector();
    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    assert!(
        ref_result.stars.len() >= 20,
        "Not enough stars detected in reference: {}",
        ref_result.stars.len()
    );
    assert!(
        target_result.stars.len() >= 20,
        "Not enough stars detected in target: {}",
        target_result.stars.len()
    );

    // Register the images using detected stars directly
    let reg_config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        max_rms_error: 3.0,
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    // Verify the recovered translation
    let recovered = result.transform.translation_components();
    let recovered_dx = recovered.x;
    let recovered_dy = recovered.y;

    let dx_error = (recovered_dx - dx).abs();
    let dy_error = (recovered_dy - dy).abs();

    assert!(
        dx_error < 1.0,
        "X translation error too large: expected {}, got {}, error {}",
        dx,
        recovered_dx,
        dx_error
    );
    assert!(
        dy_error < 1.0,
        "Y translation error too large: expected {}, got {}, error {}",
        dy,
        recovered_dy,
        dy_error
    );

    assert!(
        result.rms_error < 2.0,
        "RMS error too large: {}",
        result.rms_error
    );
}

#[test]
fn test_image_registration_rotation() {
    let config = StarFieldConfig {
        num_stars: 60,
        seed: 123,
        ..synthetic::sparse_field_config()
    };
    let width = config.width;
    let height = config.height;

    let (ref_pixels, _) = synthetic::generate_star_field(&config);
    let ref_pixels_vec = ref_pixels.into_vec();

    // Apply rotation + small translation
    let dx = 5.0;
    let dy = -3.0;
    let angle_deg: f64 = 1.0;
    let angle_rad = angle_deg.to_radians();

    let target_pixels = transform_image(&ref_pixels_vec, width, height, dx, dy, angle_rad, 1.0);

    let ref_image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), target_pixels);

    let mut det = detector();
    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    let reg_config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 6,
        min_matches: 4,
        max_rms_error: 3.0,
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    // Verify rotation recovery
    let recovered_angle = result.transform.rotation_angle();
    let rotation_error = (recovered_angle - angle_rad).abs();

    assert!(
        rotation_error < 0.02,
        "Rotation error too large: expected {} rad, got {} rad, error {}",
        angle_rad,
        recovered_angle,
        rotation_error
    );

    assert!(
        result.rms_error < 2.0,
        "RMS error too large: {}",
        result.rms_error
    );
}

#[test]
fn test_image_registration_similarity() {
    let config = StarFieldConfig {
        num_stars: 70,
        seed: 456,
        ..synthetic::sparse_field_config()
    };
    let width = config.width;
    let height = config.height;

    let (ref_pixels, _) = synthetic::generate_star_field(&config);
    let ref_pixels_vec = ref_pixels.into_vec();

    // Apply similarity transform (translation + rotation + scale)
    let dx = 8.0;
    let dy = -6.0;
    let angle_deg: f64 = 0.8;
    let angle_rad = angle_deg.to_radians();
    let scale = 1.005;

    let target_pixels = transform_image(&ref_pixels_vec, width, height, dx, dy, angle_rad, scale);

    let ref_image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), target_pixels);

    let mut det = detector();
    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    let reg_config = Config {
        transform_type: TransformType::Similarity,
        min_stars: 6,
        min_matches: 4,
        max_rms_error: 3.0,
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    // Verify scale and rotation recovery
    let recovered_scale = result.transform.scale_factor();
    let scale_error = (recovered_scale - scale).abs();

    assert!(
        scale_error < 0.005,
        "Scale error too large: expected {}, got {}, error {}",
        scale,
        recovered_scale,
        scale_error
    );

    let recovered_angle = result.transform.rotation_angle();
    let rotation_error = (recovered_angle - angle_rad).abs();

    assert!(
        rotation_error < 0.02,
        "Rotation error too large: expected {} rad, got {} rad",
        angle_rad,
        recovered_angle
    );

    assert!(
        result.rms_error < 2.0,
        "RMS error too large: {}",
        result.rms_error
    );
}

#[test]
fn test_image_registration_with_noise() {
    // Higher noise level
    let config = StarFieldConfig {
        num_stars: 80,
        noise_sigma: 0.04, // Higher noise
        seed: 789,
        ..synthetic::sparse_field_config()
    };
    let width = config.width;
    let height = config.height;

    let (ref_pixels, _) = synthetic::generate_star_field(&config);
    let ref_pixels_vec = ref_pixels.into_vec();

    // Apply translation
    let dx = 20.0;
    let dy = -15.0;
    let target_pixels = translate_image(&ref_pixels_vec, width, height, dx, dy);

    let ref_image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), target_pixels);

    let mut det = StarDetector::from_config(DetConfig {
        expected_fwhm: 0.0,
        min_snr: 8.0,
        sigma_threshold: 4.0,
        ..Default::default()
    });

    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    let reg_config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        max_rms_error: 5.0, // Allow more error due to noise
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    let recovered = result.transform.translation_components();
    let recovered_dx = recovered.x;
    let recovered_dy = recovered.y;

    // Allow more tolerance due to noise
    let dx_error = (recovered_dx - dx).abs();
    let dy_error = (recovered_dy - dy).abs();

    assert!(
        dx_error < 2.0,
        "X translation error too large: expected {}, got {}",
        dx,
        recovered_dx
    );
    assert!(
        dy_error < 2.0,
        "Y translation error too large: expected {}, got {}",
        dy,
        recovered_dy
    );
}

#[test]
fn test_image_registration_dense_field() {
    // Dense star field
    let config = StarFieldConfig {
        num_stars: 200,
        seed: 999,
        ..synthetic::dense_field_config()
    };
    let width = config.width;
    let height = config.height;

    let (ref_pixels, _) = synthetic::generate_star_field(&config);
    let ref_pixels_vec = ref_pixels.into_vec();

    let dx = 10.0;
    let dy = 8.0;
    let angle_rad = 0.5_f64.to_radians();

    let target_pixels = transform_image(&ref_pixels_vec, width, height, dx, dy, angle_rad, 1.0);

    let ref_image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), target_pixels);

    let mut det = detector();
    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    assert!(
        ref_result.stars.len() >= 50,
        "Expected many stars in dense field, got {}",
        ref_result.stars.len()
    );

    let reg_config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 10,
        min_matches: 8,
        max_rms_error: 3.0,
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    // Should have many matched stars in a dense field
    assert!(
        result.num_inliers >= 20,
        "Expected many inliers in dense field, got {}",
        result.num_inliers
    );

    assert!(
        result.rms_error < 2.0,
        "RMS error too large: {}",
        result.rms_error
    );
}

#[test]
fn test_image_registration_large_image() {
    let config = StarFieldConfig {
        width: 1024,
        height: 1024,
        num_stars: 100,
        seed: 111,
        ..synthetic::sparse_field_config()
    };
    let width = config.width;
    let height = config.height;

    let (ref_pixels, _) = synthetic::generate_star_field(&config);
    let ref_pixels_vec = ref_pixels.into_vec();

    // Larger translation for larger image
    let dx = 50.0;
    let dy = -35.0;
    let target_pixels = translate_image(&ref_pixels_vec, width, height, dx, dy);

    let ref_image = AstroImage::from_pixels(ImageDimensions::new(width, height, 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), target_pixels);

    let mut det = detector();
    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    let reg_config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        max_rms_error: 3.0,
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    let recovered = result.transform.translation_components();
    let recovered_dx = recovered.x;
    let recovered_dy = recovered.y;

    let dx_error = (recovered_dx - dx).abs();
    let dy_error = (recovered_dy - dy).abs();

    assert!(
        dx_error < 1.0,
        "X translation error too large: {}",
        dx_error
    );
    assert!(
        dy_error < 1.0,
        "Y translation error too large: {}",
        dy_error
    );
}
