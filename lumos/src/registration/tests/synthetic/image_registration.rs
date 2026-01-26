//! End-to-end registration tests using synthetic star field images.
//!
//! These tests generate actual pixel images with synthetic stars,
//! run star detection on both images, and verify that registration
//! correctly recovers the applied transformation.

use crate::AstroImage;
use crate::astro_image::{AstroImageMetadata, ImageDimensions};
use crate::registration::{RegistrationConfig, Registrator};
use crate::star_detection::{StarDetectionConfig, find_stars};
use crate::testing::synthetic::{self, StarFieldConfig};

/// Default star detection config for synthetic images.
fn detection_config() -> StarDetectionConfig {
    StarDetectionConfig {
        expected_fwhm: 0.0, // Disable matched filter for synthetic images
        detection_sigma: 3.0,
        min_snr: 5.0,
        ..Default::default()
    }
}

/// Create an AstroImage from pixel data.
fn create_astro_image(pixels: Vec<f32>, width: usize, height: usize) -> AstroImage {
    AstroImage {
        pixels,
        dimensions: ImageDimensions::new(width, height, 1),
        metadata: AstroImageMetadata::default(),
    }
}

/// Apply a similarity transform to an image by resampling.
/// This creates a new image where each pixel is sampled from the source
/// at the inverse-transformed position.
#[allow(clippy::too_many_arguments)]
fn transform_image(
    src_pixels: &[f32],
    width: usize,
    height: usize,
    dx: f64,
    dy: f64,
    angle_rad: f64,
    scale: f64,
    center_x: f64,
    center_y: f64,
) -> Vec<f32> {
    let mut dst_pixels = vec![0.0f32; width * height];

    // For each destination pixel, find the source pixel
    for dst_y in 0..height {
        for dst_x in 0..width {
            // Inverse transform: map destination back to source
            // Forward: x' = cos*rx - sin*ry + cx + dx, where rx = x - cx
            // Inverse: need to solve for (x, y) given (x', y')
            let x_prime = dst_x as f64;
            let y_prime = dst_y as f64;

            // Remove translation
            let tx = x_prime - center_x - dx;
            let ty = y_prime - center_y - dy;

            // Inverse rotation and scale
            let inv_scale = 1.0 / scale;
            let inv_cos = angle_rad.cos() * inv_scale;
            let inv_sin = angle_rad.sin() * inv_scale;

            let src_x = inv_cos * tx + inv_sin * ty + center_x;
            let src_y = -inv_sin * tx + inv_cos * ty + center_y;

            // Bilinear interpolation
            if src_x >= 0.0
                && src_x < (width - 1) as f64
                && src_y >= 0.0
                && src_y < (height - 1) as f64
            {
                let x0 = src_x.floor() as usize;
                let y0 = src_y.floor() as usize;
                let x1 = x0 + 1;
                let y1 = y0 + 1;

                let fx = src_x - x0 as f64;
                let fy = src_y - y0 as f64;

                let p00 = src_pixels[y0 * width + x0] as f64;
                let p10 = src_pixels[y0 * width + x1] as f64;
                let p01 = src_pixels[y1 * width + x0] as f64;
                let p11 = src_pixels[y1 * width + x1] as f64;

                let value = p00 * (1.0 - fx) * (1.0 - fy)
                    + p10 * fx * (1.0 - fy)
                    + p01 * (1.0 - fx) * fy
                    + p11 * fx * fy;

                dst_pixels[dst_y * width + dst_x] = value as f32;
            }
        }
    }

    dst_pixels
}

/// Apply a translation to an image.
fn translate_image(src_pixels: &[f32], width: usize, height: usize, dx: f64, dy: f64) -> Vec<f32> {
    transform_image(
        src_pixels,
        width,
        height,
        dx,
        dy,
        0.0,
        1.0,
        width as f64 / 2.0,
        height as f64 / 2.0,
    )
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

    // Apply a known translation to create target image
    let dx = 15.5;
    let dy = -12.3;
    let target_pixels = translate_image(&ref_pixels, width, height, dx, dy);

    // Create AstroImages
    let ref_image = create_astro_image(ref_pixels, width, height);
    let target_image = create_astro_image(target_pixels, width, height);

    // Detect stars in both images
    let det_config = detection_config();
    let ref_result = find_stars(&ref_image, &det_config);
    let target_result = find_stars(&target_image, &det_config);

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

    // Convert detected stars to (x, y) tuples
    let ref_stars: Vec<(f64, f64)> = ref_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();
    let target_stars: Vec<(f64, f64)> = target_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();

    // Register the images
    let reg_config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(reg_config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    // Verify the recovered translation
    let (recovered_dx, recovered_dy) = result.transform.translation_components();

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

    // Apply rotation + small translation
    let dx = 5.0;
    let dy = -3.0;
    let angle_deg: f64 = 1.0;
    let angle_rad = angle_deg.to_radians();
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;

    let target_pixels = transform_image(
        &ref_pixels,
        width,
        height,
        dx,
        dy,
        angle_rad,
        1.0,
        center_x,
        center_y,
    );

    let ref_image = create_astro_image(ref_pixels, width, height);
    let target_image = create_astro_image(target_pixels, width, height);

    let det_config = detection_config();
    let ref_result = find_stars(&ref_image, &det_config);
    let target_result = find_stars(&target_image, &det_config);

    let ref_stars: Vec<(f64, f64)> = ref_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();
    let target_stars: Vec<(f64, f64)> = target_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();

    let reg_config = RegistrationConfig::builder()
        .with_rotation()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(reg_config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
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

    // Apply similarity transform (translation + rotation + scale)
    let dx = 8.0;
    let dy = -6.0;
    let angle_deg: f64 = 0.8;
    let angle_rad = angle_deg.to_radians();
    let scale = 1.005;
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;

    let target_pixels = transform_image(
        &ref_pixels,
        width,
        height,
        dx,
        dy,
        angle_rad,
        scale,
        center_x,
        center_y,
    );

    let ref_image = create_astro_image(ref_pixels, width, height);
    let target_image = create_astro_image(target_pixels, width, height);

    let det_config = detection_config();
    let ref_result = find_stars(&ref_image, &det_config);
    let target_result = find_stars(&target_image, &det_config);

    let ref_stars: Vec<(f64, f64)> = ref_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();
    let target_stars: Vec<(f64, f64)> = target_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();

    let reg_config = RegistrationConfig::builder()
        .with_scale()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(reg_config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
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

    // Apply translation
    let dx = 20.0;
    let dy = -15.0;
    let target_pixels = translate_image(&ref_pixels, width, height, dx, dy);

    let ref_image = create_astro_image(ref_pixels, width, height);
    let target_image = create_astro_image(target_pixels, width, height);

    let det_config = StarDetectionConfig {
        expected_fwhm: 0.0,
        detection_sigma: 4.0, // Higher threshold for noisy image
        min_snr: 8.0,
        ..Default::default()
    };

    let ref_result = find_stars(&ref_image, &det_config);
    let target_result = find_stars(&target_image, &det_config);

    let ref_stars: Vec<(f64, f64)> = ref_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();
    let target_stars: Vec<(f64, f64)> = target_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();

    let reg_config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(5.0) // Allow more error due to noise
        .build();

    let registrator = Registrator::new(reg_config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

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

    let dx = 10.0;
    let dy = 8.0;
    let angle_rad = 0.5_f64.to_radians();
    let center_x = width as f64 / 2.0;
    let center_y = height as f64 / 2.0;

    let target_pixels = transform_image(
        &ref_pixels,
        width,
        height,
        dx,
        dy,
        angle_rad,
        1.0,
        center_x,
        center_y,
    );

    let ref_image = create_astro_image(ref_pixels, width, height);
    let target_image = create_astro_image(target_pixels, width, height);

    let det_config = detection_config();
    let ref_result = find_stars(&ref_image, &det_config);
    let target_result = find_stars(&target_image, &det_config);

    assert!(
        ref_result.stars.len() >= 50,
        "Expected many stars in dense field, got {}",
        ref_result.stars.len()
    );

    let ref_stars: Vec<(f64, f64)> = ref_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();
    let target_stars: Vec<(f64, f64)> = target_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();

    let reg_config = RegistrationConfig::builder()
        .with_rotation()
        .min_stars(10)
        .min_matched_stars(8)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(reg_config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
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

    // Larger translation for larger image
    let dx = 50.0;
    let dy = -35.0;
    let target_pixels = translate_image(&ref_pixels, width, height, dx, dy);

    let ref_image = create_astro_image(ref_pixels, width, height);
    let target_image = create_astro_image(target_pixels, width, height);

    let det_config = detection_config();
    let ref_result = find_stars(&ref_image, &det_config);
    let target_result = find_stars(&target_image, &det_config);

    let ref_stars: Vec<(f64, f64)> = ref_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();
    let target_stars: Vec<(f64, f64)> = target_result
        .stars
        .iter()
        .map(|s| (s.x as f64, s.y as f64))
        .collect();

    let reg_config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(reg_config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

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
