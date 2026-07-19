//! End-to-end registration tests using synthetic star field images.
//!
//! These tests generate actual pixel images with synthetic stars,
//! run star detection on both images, and verify that registration
//! correctly recovers the applied transformation.

use crate::{AstroImage, ImageDimensions};
use glam::DVec2;

use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::interpolation::{WarpParams, warp_image};
use crate::stacking::registration::synthetic_tests::helpers;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::stacking::registration::{Config, TransformType, register};
use crate::stacking::star_detection::config::Config as DetConfig;
use crate::stacking::star_detection::detector::StarDetector;
use crate::testing::synthetic::camera::Camera;
use crate::testing::synthetic::fixtures::star_field;
use crate::testing::synthetic::observe::{Observation, render};
use crate::testing::synthetic::scene::{BackgroundField, Scene};
use imaginarium::Buffer2;

/// Default star detector for synthetic images.
fn detector() -> StarDetector {
    let mut config = DetConfig::default();
    config.fwhm.expected = 0.0;
    config.filter.min_snr = 5.0;
    config.detection.sigma_threshold = 3.0;
    StarDetector::from_config(config).unwrap()
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
    let mut output = Buffer2::new_default(width, height);
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
    let mut output = Buffer2::new_default(width, height);
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
    // Reference star field image (forward model).
    let (width, height) = (256, 256);
    let ref_pixels_vec = star_field(width, height, 50, 42)
        .image
        .channel(0)
        .pixels()
        .to_vec();

    // Apply a known translation to create target image
    let dx = 15.5;
    let dy = -12.3;
    let target_pixels = translate_image(&ref_pixels_vec, width, height, dx, dy);

    // Create AstroImages
    let ref_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), target_pixels);

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
        matching: helpers::matching_config(6, 4),
        max_rms_error: 3.0,
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    // Verify the recovered translation
    let recovered = result.transform().translation_components();
    let recovered_dx = recovered.x;
    let recovered_dy = recovered.y;

    let dx_error = (recovered_dx - dx).abs();
    let dy_error = (recovered_dy - dy).abs();

    // Noiseless fixture: recovery is limited only by centroid scatter, so the gate is sub-pixel.
    assert!(
        dx_error < 0.3,
        "X translation error too large: expected {}, got {}, error {}",
        dx,
        recovered_dx,
        dx_error
    );
    assert!(
        dy_error < 0.3,
        "Y translation error too large: expected {}, got {}, error {}",
        dy,
        recovered_dy,
        dy_error
    );

    assert!(
        result.rms_error() < 0.5,
        "RMS error too large: {}",
        result.rms_error()
    );
}

#[test]
fn test_image_registration_rotation() {
    let (width, height) = (256, 256);
    let ref_pixels_vec = star_field(width, height, 60, 123)
        .image
        .channel(0)
        .pixels()
        .to_vec();

    // Apply rotation + small translation
    let dx = 5.0;
    let dy = -3.0;
    let angle_deg: f64 = 1.0;
    let angle_rad = angle_deg.to_radians();

    let target_pixels = transform_image(&ref_pixels_vec, width, height, dx, dy, angle_rad, 1.0);

    let ref_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), target_pixels);

    let mut det = detector();
    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    let reg_config = Config {
        transform_type: TransformType::Euclidean,
        matching: helpers::matching_config(6, 4),
        max_rms_error: 3.0,
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    // Verify rotation recovery
    let recovered_angle = result.transform().rotation_angle();
    let rotation_error = (recovered_angle - angle_rad).abs();

    assert!(
        rotation_error < 0.02,
        "Rotation error too large: expected {} rad, got {} rad, error {}",
        angle_rad,
        recovered_angle,
        rotation_error
    );

    assert!(
        result.rms_error() < 0.5,
        "RMS error too large: {}",
        result.rms_error()
    );
}

#[test]
fn test_image_registration_similarity() {
    let (width, height) = (256, 256);
    let ref_pixels_vec = star_field(width, height, 70, 456)
        .image
        .channel(0)
        .pixels()
        .to_vec();

    // Apply similarity transform (translation + rotation + scale)
    let dx = 8.0;
    let dy = -6.0;
    let angle_deg: f64 = 0.8;
    let angle_rad = angle_deg.to_radians();
    let scale = 1.005;

    let target_pixels = transform_image(&ref_pixels_vec, width, height, dx, dy, angle_rad, scale);

    let ref_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), target_pixels);

    let mut det = detector();
    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    let reg_config = Config {
        transform_type: TransformType::Similarity,
        matching: helpers::matching_config(6, 4),
        max_rms_error: 3.0,
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    // Verify scale and rotation recovery
    let recovered_scale = result.transform().scale_factor();
    let scale_error = (recovered_scale - scale).abs();

    assert!(
        scale_error < 0.005,
        "Scale error too large: expected {}, got {}, error {}",
        scale,
        recovered_scale,
        scale_error
    );

    let recovered_angle = result.transform().rotation_angle();
    let rotation_error = (recovered_angle - angle_rad).abs();

    assert!(
        rotation_error < 0.02,
        "Rotation error too large: expected {} rad, got {} rad",
        angle_rad,
        recovered_angle
    );

    assert!(
        result.rms_error() < 2.0,
        "RMS error too large: {}",
        result.rms_error()
    );
}

#[test]
fn test_image_registration_with_noise() {
    // Higher noise level: a shallow well + extra read noise stresses registration.
    let (width, height) = (256, 256);
    let scene = Scene::random_field(
        width,
        height,
        80,
        (6.0, 16.0),
        BackgroundField::Uniform { level: 0.1 },
        16.0,
        789,
    );
    let noisy = Camera {
        full_well_e: 3000.0,
        read_noise_e: 15.0,
        ..Camera::realistic(4.0)
    };
    let ref_pixels_vec = render(&scene, &noisy, &Observation::reference(789))
        .image
        .channel(0)
        .pixels()
        .to_vec();

    // Apply translation
    let dx = 20.0;
    let dy = -15.0;
    let target_pixels = translate_image(&ref_pixels_vec, width, height, dx, dy);

    let ref_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), target_pixels);

    let mut detection_config = DetConfig::default();
    detection_config.fwhm.expected = 0.0;
    detection_config.filter.min_snr = 8.0;
    detection_config.detection.sigma_threshold = 4.0;
    let mut det = StarDetector::from_config(detection_config).unwrap();

    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    let reg_config = Config {
        transform_type: TransformType::Translation,
        matching: helpers::matching_config(6, 4),
        max_rms_error: 5.0, // Allow more error due to noise
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    let recovered = result.transform().translation_components();
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
    // Dense star field.
    let (width, height) = (256, 256);
    let ref_pixels_vec = star_field(width, height, 200, 999)
        .image
        .channel(0)
        .pixels()
        .to_vec();

    let dx = 10.0;
    let dy = 8.0;
    let angle_rad = 0.5_f64.to_radians();

    let target_pixels = transform_image(&ref_pixels_vec, width, height, dx, dy, angle_rad, 1.0);

    let ref_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), target_pixels);

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
        matching: helpers::matching_config(10, 8),
        max_rms_error: 3.0,
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    // Should have many matched stars in a dense field
    assert!(
        result.num_inliers() >= 20,
        "Expected many inliers in dense field, got {}",
        result.num_inliers()
    );

    assert!(
        result.rms_error() < 2.0,
        "RMS error too large: {}",
        result.rms_error()
    );
}

#[test]
fn test_image_registration_large_image() {
    let (width, height) = (1024, 1024);
    let ref_pixels_vec = star_field(width, height, 100, 111)
        .image
        .channel(0)
        .pixels()
        .to_vec();

    // Larger translation for larger image
    let dx = 50.0;
    let dy = -35.0;
    let target_pixels = translate_image(&ref_pixels_vec, width, height, dx, dy);

    let ref_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), ref_pixels_vec);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new((width, height), 1), target_pixels);

    let mut det = detector();
    let ref_result = det.detect(&ref_image);
    let target_result = det.detect(&target_image);

    let reg_config = Config {
        transform_type: TransformType::Translation,
        matching: helpers::matching_config(6, 4),
        max_rms_error: 3.0,
        ..Default::default()
    };

    let result = register(&ref_result.stars, &target_result.stars, &reg_config)
        .expect("Registration should succeed");

    let recovered = result.transform().translation_components();
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
