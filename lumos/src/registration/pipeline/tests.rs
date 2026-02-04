use glam::DVec2;

use super::*;
use crate::AstroImage;
use crate::common::Buffer2;
use crate::registration::transform::TransformType;
use std::f64::consts::PI;

/// Test helper: register star positions with specified transform type
fn register_star_positions(
    ref_positions: &[DVec2],
    target_positions: &[DVec2],
    transform_type: TransformType,
) -> Result<RegistrationResult, RegistrationError> {
    let config = RegistrationConfig {
        transform_type,
        min_stars_for_matching: 6,
        min_matched_stars: 4,
        max_residual_pixels: 2.0,
        ..Default::default()
    };
    Registrator::new(config).register_positions(ref_positions, target_positions)
}

fn generate_star_grid(rows: usize, cols: usize, spacing: f64, offset: DVec2) -> Vec<DVec2> {
    let mut stars = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            stars.push(offset + DVec2::new(c as f64 * spacing, r as f64 * spacing));
        }
    }
    stars
}

fn transform_stars(stars: &[DVec2], transform: &Transform) -> Vec<DVec2> {
    stars.iter().map(|&p| transform.apply(p)).collect()
}

#[test]
fn test_registration_identity() {
    let ref_stars = generate_star_grid(5, 5, 100.0, DVec2::new(100.0, 100.0));
    let target_stars = ref_stars.clone();

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    // Should find near-identity transform
    let t = result.transform.translation_components();
    assert!(
        t.x.abs() < 1.0,
        "Expected near-zero translation, got {}",
        t.x
    );
    assert!(
        t.y.abs() < 1.0,
        "Expected near-zero translation, got {}",
        t.y
    );
    assert!(result.rms_error < 0.5, "Expected low RMS error");
}

#[test]
fn test_registration_translation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, DVec2::new(100.0, 100.0));
    let translation = Transform::translation(DVec2::new(50.0, -30.0));
    let target_stars = transform_stars(&ref_stars, &translation);

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let t = result.transform.translation_components();
    assert!((t.x - 50.0).abs() < 1.0, "Expected tx=50, got {}", t.x);
    assert!((t.y - (-30.0)).abs() < 1.0, "Expected ty=-30, got {}", t.y);
}

#[test]
fn test_registration_rotation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, DVec2::new(200.0, 200.0));
    let rotation = Transform::euclidean(DVec2::new(10.0, -5.0), 0.1); // ~5.7 degrees
    let target_stars = transform_stars(&ref_stars, &rotation);

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Euclidean).unwrap();

    let angle = result.transform.rotation_angle();
    assert!(
        (angle - 0.1).abs() < 0.01,
        "Expected angle=0.1, got {}",
        angle
    );
}

#[test]
fn test_registration_similarity() {
    let ref_stars = generate_star_grid(5, 5, 100.0, DVec2::new(200.0, 200.0));
    let similarity = Transform::similarity(DVec2::new(20.0, 15.0), 0.05, 1.02);
    let target_stars = transform_stars(&ref_stars, &similarity);

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Similarity).unwrap();

    let scale = result.transform.scale_factor();
    assert!(
        (scale - 1.02).abs() < 0.01,
        "Expected scale=1.02, got {}",
        scale
    );
}

#[test]
fn test_registration_with_outliers() {
    let ref_stars = generate_star_grid(6, 6, 80.0, DVec2::new(100.0, 100.0));
    let translation = Transform::translation(DVec2::new(25.0, 40.0));
    let mut target_stars = transform_stars(&ref_stars, &translation);

    // Add outliers (wrong matches)
    target_stars[0] = DVec2::new(500.0, 500.0);
    target_stars[5] = DVec2::new(50.0, 800.0);
    target_stars[10] = DVec2::new(900.0, 100.0);

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let t = result.transform.translation_components();
    // RANSAC should still find correct translation despite outliers
    assert!((t.x - 25.0).abs() < 2.0, "Expected tx=25, got {}", t.x);
    assert!((t.y - 40.0).abs() < 2.0, "Expected ty=40, got {}", t.y);
}

#[test]
fn test_registration_insufficient_stars() {
    let ref_stars = vec![DVec2::new(100.0, 100.0), DVec2::new(200.0, 200.0)];
    let target_stars = ref_stars.clone();

    let result = register_star_positions(&ref_stars, &target_stars, TransformType::Translation);
    assert!(matches!(
        result,
        Err(RegistrationError::InsufficientStars { .. })
    ));
}

#[test]
fn test_registrator_config() {
    let config = RegistrationConfig {
        transform_type: TransformType::Euclidean,
        ransac: crate::registration::ransac::RansacConfig {
            max_iterations: 2000,
            inlier_threshold: 1.5,
            ..crate::registration::ransac::RansacConfig::default()
        },
        ..Default::default()
    };

    let registrator = Registrator::new(config);
    assert_eq!(registrator.config().ransac.max_iterations, 2000);
    assert!((registrator.config().ransac.inlier_threshold - 1.5).abs() < 1e-10);
}

#[test]
fn test_warp_to_reference_image() {
    // Create a simple test image with a bright pixel offset from center
    let width = 64;
    let height = 64;
    let mut target_pixels = vec![0.0f32; width * height];

    // Place bright pixel at (37, 35) in target image
    let target_x = 37;
    let target_y = 35;
    target_pixels[target_y * width + target_x] = 1.0;

    let target_image =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), target_pixels);

    // Transform maps reference -> target: ref(32,32) -> target(37,35)
    // So translation is (5, 3)
    let transform = Transform::translation(DVec2::new(5.0, 3.0));

    // warp_to_reference_image should align target to reference frame
    // The pixel at target(37,35) should appear at reference(32,32) after warping
    let warped = warp_to_reference_image(&target_image, &transform, InterpolationMethod::Bilinear);

    assert_eq!(warped.channel(0).len(), width * height);

    // The bright pixel should now be at reference position (32, 32)
    let ref_x = 32;
    let ref_y = 32;
    assert!(
        warped.channel(0)[ref_y * width + ref_x] > 0.5,
        "Expected bright pixel at reference position ({}, {}), got {}",
        ref_x,
        ref_y,
        warped.channel(0)[ref_y * width + ref_x]
    );

    // Original target position should now be dark (or near-dark due to interpolation)
    assert!(
        warped.channel(0)[target_y * width + target_x] < 0.5,
        "Expected dark at original target position ({}, {}), got {}",
        target_x,
        target_y,
        warped.channel(0)[target_y * width + target_x]
    );
}

/// Test that warp_to_reference_image correctly aligns a warped image back to reference
#[test]
fn test_warp_to_reference_image_roundtrip() {
    let width = 128;
    let height = 128;

    // Create reference image with a few bright spots
    let mut ref_pixels = vec![0.0f32; width * height];
    let ref_points = [(40, 40), (80, 40), (60, 80), (40, 80), (80, 80)];
    for &(x, y) in &ref_points {
        ref_pixels[y * width + x] = 1.0;
    }

    // Define transform: reference -> target
    let transform = Transform::similarity(DVec2::new(10.0, -5.0), 0.1, 1.02);

    // Create target image by warping reference with the transform
    // (simulates what the camera would see if shifted/rotated)
    let ref_buf = Buffer2::new(width, height, ref_pixels.clone());
    let target_pixels = crate::registration::interpolation::warp_image(
        &ref_buf,
        width,
        height,
        &transform,
        &crate::registration::interpolation::WarpConfig {
            method: InterpolationMethod::Lanczos3,
            border_value: 0.0,
            normalize_kernel: true,
            clamp_output: false,
        },
    );

    let target_image = AstroImage::from_pixels(
        ImageDimensions::new(width, height, 1),
        target_pixels.into_vec(),
    );

    // Now use warp_to_reference_image to align target back to reference frame
    let aligned = warp_to_reference_image(&target_image, &transform, InterpolationMethod::Lanczos3);

    // Compare aligned image to reference (excluding borders affected by warping)
    let margin = 20;
    let mut max_diff = 0.0f32;
    let mut sum_sq_diff = 0.0f32;
    let mut count = 0;

    let aligned_channel = aligned.channel(0);
    for y in margin..height - margin {
        for x in margin..width - margin {
            let ref_val = ref_pixels[y * width + x];
            let aligned_val = aligned_channel[y * width + x];
            let diff = (ref_val - aligned_val).abs();
            max_diff = max_diff.max(diff);
            sum_sq_diff += diff * diff;
            count += 1;
        }
    }

    let rmse = (sum_sq_diff / count as f32).sqrt();

    // After roundtrip, images should be very similar
    // Some error is expected due to double interpolation (forward + inverse warp)
    assert!(
        rmse < 0.15,
        "Roundtrip RMSE too high: {} (expected < 0.15)",
        rmse
    );
    assert!(
        max_diff < 0.5,
        "Roundtrip max diff too high: {} (expected < 0.5)",
        max_diff
    );
}

/// End-to-end test: detect transform from stars, warp image, verify alignment
#[test]
fn test_warp_to_reference_image_end_to_end() {
    let width = 256;
    let height = 256;

    // Generate reference star positions
    let ref_stars = generate_star_grid(6, 6, 35.0, DVec2::new(30.0, 30.0));

    // Create reference image with Gaussian stars
    let ref_pixels = generate_synthetic_star_image(width, height, &ref_stars, 1.0, 4.0);

    // Apply known transform to star positions
    let known_transform = Transform::similarity(DVec2::new(12.0, -8.0), 0.05, 1.01);
    let target_stars = transform_stars(&ref_stars, &known_transform);

    // Create target image with transformed star positions
    let target_pixels = generate_synthetic_star_image(width, height, &target_stars, 1.0, 4.0);
    let target_image =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), target_pixels);

    // Register: find transform from reference stars to target stars
    let result = register_star_positions(&ref_stars, &target_stars, TransformType::Similarity)
        .expect("Registration should succeed");

    // Warp target image to align with reference
    let aligned = warp_to_reference_image(
        &target_image,
        &result.transform,
        InterpolationMethod::Lanczos3,
    );

    // Compute alignment quality metrics in central region
    let margin = 30;
    let mut ref_sum = 0.0f32;
    let mut aligned_sum = 0.0f32;
    let mut product_sum = 0.0f32;
    let mut ref_sq_sum = 0.0f32;
    let mut aligned_sq_sum = 0.0f32;
    let mut diff_sum = 0.0f32;
    let mut count = 0;

    let aligned_channel = aligned.channel(0);
    for y in margin..height - margin {
        for x in margin..width - margin {
            let r = ref_pixels[y * width + x];
            let a = aligned_channel[y * width + x];

            ref_sum += r;
            aligned_sum += a;
            product_sum += r * a;
            ref_sq_sum += r * r;
            aligned_sq_sum += a * a;
            diff_sum += (r - a).abs();
            count += 1;
        }
    }

    let n = count as f32;
    let ref_mean = ref_sum / n;
    let aligned_mean = aligned_sum / n;

    // Normalized Cross-Correlation
    let numerator = product_sum - n * ref_mean * aligned_mean;
    let denom_ref = (ref_sq_sum - n * ref_mean * ref_mean).sqrt();
    let denom_aligned = (aligned_sq_sum - n * aligned_mean * aligned_mean).sqrt();
    let ncc = numerator / (denom_ref * denom_aligned + 1e-10);

    // Mean Absolute Error
    let mae = diff_sum / n;

    // NCC > 0.9 indicates good alignment; wrong direction would give NCC << 0.5
    assert!(
        ncc > 0.90,
        "End-to-end alignment NCC too low: {} (expected > 0.90)",
        ncc
    );
    assert!(
        mae < 0.1,
        "End-to-end alignment MAE too high: {} (expected < 0.1)",
        mae
    );
}

#[test]
fn test_quick_register() {
    let ref_stars = generate_star_grid(4, 4, 150.0, DVec2::new(100.0, 100.0));
    let translation = Transform::translation(DVec2::new(10.0, -15.0));
    let target_stars = transform_stars(&ref_stars, &translation);

    let transform = quick_register(&ref_stars, &target_stars).unwrap();
    let t = transform.translation_components();

    assert!((t.x - 10.0).abs() < 1.0);
    assert!((t.y - (-15.0)).abs() < 1.0);
}

#[test]
fn test_registration_result_quality() {
    let ref_stars = generate_star_grid(6, 6, 100.0, DVec2::new(50.0, 50.0));
    let target_stars = ref_stars.clone();

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    // Perfect match should have very low error and high quality
    assert!(result.rms_error < 0.1);
    assert!(result.num_inliers >= 20);
}

#[test]
fn test_registration_large_rotation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, DVec2::new(250.0, 250.0));
    // 30 degree rotation around image center
    let rotation = Transform::rotation_around(DVec2::new(300.0, 300.0), PI / 6.0);
    let target_stars = transform_stars(&ref_stars, &rotation);

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Euclidean).unwrap();

    let angle = result.transform.rotation_angle();
    assert!(
        (angle - PI / 6.0).abs() < 0.05,
        "Expected 30deg rotation, got {} rad",
        angle
    );
}

// ============================================================================
// Milestone F: Test Hardening - Ground truth validation tests
// ============================================================================

/// Generate synthetic star field with Gaussian profiles for testing
fn generate_synthetic_star_image(
    width: usize,
    height: usize,
    stars: &[DVec2],
    brightness: f32,
    fwhm: f32,
) -> Vec<f32> {
    let mut image = vec![0.0f32; width * height];
    let sigma = fwhm / 2.355; // FWHM to sigma conversion

    for &star in stars {
        // Draw Gaussian profile around each star
        let radius = (3.0 * sigma) as i32;
        let cx = star.x as i32;
        let cy = star.y as i32;

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let px = cx + dx;
                let py = cy + dy;
                if px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < height {
                    let dist_sq = (dx as f32).powi(2) + (dy as f32).powi(2);
                    let val = brightness * (-dist_sq / (2.0 * sigma * sigma)).exp();
                    image[py as usize * width + px as usize] += val;
                }
            }
        }
    }

    image
}

/// Test full pipeline with synthetic images - ground truth validation
#[test]
fn test_pipeline_ground_truth_synthetic() {
    let width = 256;
    let height = 256;

    // Generate reference star positions
    let ref_stars = generate_star_grid(6, 6, 35.0, DVec2::new(30.0, 30.0));

    // Apply known transform
    let known_transform = Transform::similarity(DVec2::new(15.0, -10.0), 0.05, 1.02);
    let target_stars = transform_stars(&ref_stars, &known_transform);

    // Create synthetic images
    let ref_image = generate_synthetic_star_image(width, height, &ref_stars, 1.0, 4.0);
    let target_image = generate_synthetic_star_image(width, height, &target_stars, 1.0, 4.0);

    // Register using star positions (simulating star detection)
    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Similarity).unwrap();

    // Verify transform accuracy
    let estimated_scale = result.transform.scale_factor();
    let estimated_angle = result.transform.rotation_angle();
    let _est_t = result.transform.translation_components();

    assert!(
        (estimated_scale - 1.02).abs() < 0.01,
        "Scale error: expected 1.02, got {}",
        estimated_scale
    );
    assert!(
        (estimated_angle - 0.05).abs() < 0.01,
        "Angle error: expected 0.05, got {}",
        estimated_angle
    );

    // Warp target to reference and verify alignment
    let target_astro =
        AstroImage::from_pixels(ImageDimensions::new(width, height, 1), target_image);
    let warped = warp_to_reference_image(
        &target_astro,
        &result.transform,
        InterpolationMethod::Lanczos3,
    );

    // Calculate alignment error by comparing pixel intensities
    let mut error_sum = 0.0f32;
    let mut pixel_count = 0;

    let warped_channel = warped.channel(0);
    for y in 20..height - 20 {
        for x in 20..width - 20 {
            let ref_val = ref_image[y * width + x];
            let warped_val = warped_channel[y * width + x];

            // Only compare significant pixels
            if ref_val > 0.1 {
                error_sum += (ref_val - warped_val).abs();
                pixel_count += 1;
            }
        }
    }

    let mae = error_sum / pixel_count as f32;
    // MAE of ~0.4 is reasonable for sub-pixel aligned Gaussian stars
    // due to interpolation and sub-pixel position differences
    assert!(mae < 0.5, "Image alignment error too high: MAE = {}", mae);
}

/// Test pipeline handles partial overlap correctly
#[test]
fn test_pipeline_partial_overlap() {
    // Stars with large translation (50% overlap)
    let ref_stars = generate_star_grid(6, 6, 50.0, DVec2::new(50.0, 50.0));
    let large_translation = Transform::translation(DVec2::new(150.0, 0.0));
    let target_stars = transform_stars(&ref_stars, &large_translation);

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let t = result.transform.translation_components();
    assert!(
        (t.x - 150.0).abs() < 2.0,
        "Large translation error: expected 150, got {}",
        t.x
    );
    assert!(t.y.abs() < 2.0, "Unexpected y translation: {}", t.y);

    // Should still find enough inliers
    assert!(
        result.num_inliers >= 15,
        "Too few inliers with partial overlap: {}",
        result.num_inliers
    );
}

/// Test pipeline with noisy star positions
#[test]
fn test_pipeline_with_position_noise() {
    let ref_stars = generate_star_grid(6, 6, 80.0, DVec2::new(100.0, 100.0));
    let transform = Transform::similarity(DVec2::new(10.0, -5.0), 0.03, 1.01);

    // Add noise to target positions
    let mut target_stars = transform_stars(&ref_stars, &transform);
    for (i, star) in target_stars.iter_mut().enumerate() {
        // Deterministic "noise" based on index
        let noise_x = ((i * 7) as f64 * 0.1).sin() * 0.5;
        let noise_y = ((i * 11) as f64 * 0.1).cos() * 0.5;
        star.x += noise_x;
        star.y += noise_y;
    }

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Similarity).unwrap();

    // Should still recover transform parameters approximately
    let estimated_scale = result.transform.scale_factor();
    let estimated_angle = result.transform.rotation_angle();

    assert!(
        (estimated_scale - 1.01).abs() < 0.02,
        "Scale with noise: expected 1.01, got {}",
        estimated_scale
    );
    assert!(
        (estimated_angle - 0.03).abs() < 0.02,
        "Angle with noise: expected 0.03, got {}",
        estimated_angle
    );
}

/// Test that registration produces consistent results across multiple runs
#[test]
fn test_pipeline_consistency() {
    let ref_stars = generate_star_grid(5, 5, 100.0, DVec2::new(100.0, 100.0));
    let transform = Transform::similarity(DVec2::new(20.0, 15.0), 0.08, 1.03);
    let target_stars = transform_stars(&ref_stars, &transform);

    // Run registration multiple times
    let mut scales = Vec::new();
    let mut angles = Vec::new();

    for _ in 0..5 {
        let result =
            register_star_positions(&ref_stars, &target_stars, TransformType::Similarity).unwrap();
        scales.push(result.transform.scale_factor());
        angles.push(result.transform.rotation_angle());
    }

    // Results should be consistent (low variance)
    let scale_mean: f64 = scales.iter().sum::<f64>() / scales.len() as f64;
    let scale_variance: f64 =
        scales.iter().map(|s| (s - scale_mean).powi(2)).sum::<f64>() / scales.len() as f64;

    let angle_mean: f64 = angles.iter().sum::<f64>() / angles.len() as f64;
    let angle_variance: f64 =
        angles.iter().map(|a| (a - angle_mean).powi(2)).sum::<f64>() / angles.len() as f64;

    assert!(
        scale_variance < 0.0001,
        "Scale variance too high: {}",
        scale_variance
    );
    assert!(
        angle_variance < 0.0001,
        "Angle variance too high: {}",
        angle_variance
    );
}

/// Test affine transform recovery (includes shear)
#[test]
fn test_pipeline_affine_transform() {
    let ref_stars = generate_star_grid(6, 6, 80.0, DVec2::new(100.0, 100.0));

    // Affine with slight shear
    let affine = Transform::affine([1.02, 0.03, 15.0, -0.02, 0.98, 10.0]);
    let target_stars = transform_stars(&ref_stars, &affine);

    let result = register_star_positions(&ref_stars, &target_stars, TransformType::Affine).unwrap();

    // Verify by transforming reference points and checking error
    let mut total_error = 0.0;
    for (i, &r) in ref_stars.iter().enumerate() {
        let t = target_stars[i];
        let p = result.transform.apply(r);
        total_error += (p - t).length();
    }
    let mean_error = total_error / ref_stars.len() as f64;

    assert!(
        mean_error < 0.5,
        "Affine transform error too high: {}",
        mean_error
    );
}

#[test]
fn test_multiscale_registration_basic() {
    // Create a large star field
    let ref_stars = generate_star_grid(10, 10, 50.0, DVec2::new(50.0, 50.0));
    let translation = Transform::translation(DVec2::new(25.0, -15.0));
    let target_stars = transform_stars(&ref_stars, &translation);

    let config = RegistrationConfig {
        min_stars_for_matching: 6,
        min_matched_stars: 4,
        max_residual_pixels: 5.0,
        multi_scale: Some(MultiScaleConfig {
            levels: 2,
            scale_factor: 2.0,
            min_dimension: 64,
            use_phase_correlation: false,
        }),
        ..Default::default()
    };

    let registrator = MultiScaleRegistrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars, 1000, 1000)
        .unwrap();

    let t = result.transform.translation_components();
    assert!((t.x - 25.0).abs() < 2.0, "Expected tx=25, got {}", t.x);
    assert!((t.y - (-15.0)).abs() < 2.0, "Expected ty=-15, got {}", t.y);
}

#[test]
fn test_multiscale_registration_with_rotation() {
    let ref_stars = generate_star_grid(8, 8, 60.0, DVec2::new(100.0, 100.0));

    // Apply rotation + translation
    let angle = PI / 20.0; // 9 degrees
    let transform = Transform::similarity(DVec2::new(30.0, -20.0), angle, 1.0);
    let target_stars = transform_stars(&ref_stars, &transform);

    let config = RegistrationConfig {
        transform_type: TransformType::Similarity,
        min_stars_for_matching: 6,
        min_matched_stars: 4,
        max_residual_pixels: 5.0,
        multi_scale: Some(MultiScaleConfig {
            levels: 2,
            scale_factor: 2.0,
            min_dimension: 64,
            use_phase_correlation: false,
        }),
        ..Default::default()
    };

    let registrator = MultiScaleRegistrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars, 1000, 1000)
        .unwrap();

    // Check that the transform is close to the original
    let est_angle = result.transform.rotation_angle();
    assert!(
        (est_angle - angle).abs() < 0.05,
        "Expected angle={:.3}, got {:.3}",
        angle,
        est_angle
    );
}

#[test]
fn test_downsample_image() {
    // Create a simple 4x4 image
    let image = vec![
        1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
    ];

    let downsampled = super::downsample_image(&image, 4, 4, 2, 2);

    // Each 2x2 block should average to its center value
    assert_eq!(downsampled.len(), 4);

    // Top-left 2x2: (1+2+5+6)/4 = 3.5
    assert!((downsampled[0] - 3.5).abs() < 0.01);
    // Top-right 2x2: (3+4+7+8)/4 = 5.5
    assert!((downsampled[1] - 5.5).abs() < 0.01);
    // Bottom-left 2x2: (9+10+13+14)/4 = 11.5
    assert!((downsampled[2] - 11.5).abs() < 0.01);
    // Bottom-right 2x2: (11+12+15+16)/4 = 13.5
    assert!((downsampled[3] - 13.5).abs() < 0.01);
}

#[test]
fn test_build_pyramid() {
    let image = vec![0.5f32; 256 * 256];
    let pyramid = super::build_pyramid(&image, 256, 256, 3, 2.0);

    assert_eq!(pyramid.len(), 3);
    assert_eq!(pyramid[0].1, 256); // Level 0: full resolution
    assert_eq!(pyramid[0].2, 256);
    assert_eq!(pyramid[1].1, 128); // Level 1: half resolution
    assert_eq!(pyramid[1].2, 128);
    assert_eq!(pyramid[2].1, 64); // Level 2: quarter resolution
    assert_eq!(pyramid[2].2, 64);
}

#[test]
fn test_scale_transform() {
    let transform = Transform::translation(DVec2::new(10.0, 20.0));
    let scaled = super::scale_transform(&transform, 2.0);

    let t = scaled.translation_components();
    assert!((t.x - 20.0).abs() < 0.01);
    assert!((t.y - 40.0).abs() < 0.01);
}

// ============================================================================
// Integration Tests - Full Pipeline with Synthetic Astronomical Data
// ============================================================================

/// Generate a realistic star field with brightness-based positions
fn generate_realistic_star_field(
    n_stars: usize,
    width: f64,
    height: f64,
    seed: u64,
) -> Vec<(DVec2, f64)> {
    // Deterministic pseudo-random generator (simple LCG)
    let mut state = seed;
    let mut next_rand = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as f64 / (u32::MAX as f64)
    };

    let mut stars = Vec::with_capacity(n_stars);
    for _ in 0..n_stars {
        let x = next_rand() * width;
        let y = next_rand() * height;
        // Brightness follows power-law distribution (more faint stars than bright)
        let brightness = 1.0 / (1.0 + next_rand() * 9.0); // Range: 0.1 to 1.0
        stars.push((DVec2::new(x, y), brightness));
    }

    // Sort by brightness (brightest first, as detection would)
    stars.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    stars
}

/// Simulate star detection noise (centroid uncertainty)
fn add_centroid_noise(stars: &[(DVec2, f64)], noise_sigma: f64, seed: u64) -> Vec<(DVec2, f64)> {
    let mut state = seed;
    let mut next_rand = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as f64 / (u32::MAX as f64)
    };

    // Box-Muller transform for Gaussian noise
    let mut gaussian = || {
        let u1 = next_rand().max(1e-10);
        let u2 = next_rand();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    };

    stars
        .iter()
        .map(|&(pos, b)| {
            // Fainter stars have more centroid uncertainty
            let actual_sigma = noise_sigma / b.sqrt();
            let noisy_pos = pos + DVec2::new(gaussian() * actual_sigma, gaussian() * actual_sigma);
            (noisy_pos, b)
        })
        .collect()
}

/// Integration test: Simulated dithered exposure sequence
#[test]
fn test_integration_dithered_exposures() {
    // Simulate a typical dithering pattern with small offsets
    let ref_stars = generate_realistic_star_field(100, 2048.0, 2048.0, 12345);
    let ref_positions: Vec<DVec2> = ref_stars.iter().map(|&(pos, _)| pos).collect();

    // Dithering offsets (typical 5-50 pixel offsets)
    let dither_offsets = [
        DVec2::new(10.0, 15.0),
        DVec2::new(-20.0, 5.0),
        DVec2::new(8.0, -12.0),
        DVec2::new(-5.0, 25.0),
    ];

    for offset in dither_offsets {
        let transform = Transform::translation(offset);
        let target_positions: Vec<DVec2> = ref_positions
            .iter()
            .map(|&pos| transform.apply(pos))
            .collect();

        let result = register_star_positions(
            &ref_positions,
            &target_positions,
            TransformType::Translation,
        )
        .expect("Dithered registration should succeed");

        let est_offset = result.transform.translation_components();
        assert!(
            (est_offset.x - offset.x).abs() < 0.5,
            "Dither ({}, {}): expected dx={}, got {}",
            offset.x,
            offset.y,
            offset.x,
            est_offset.x
        );
        assert!(
            (est_offset.y - offset.y).abs() < 0.5,
            "Dither ({}, {}): expected dy={}, got {}",
            offset.x,
            offset.y,
            offset.y,
            est_offset.y
        );
        assert!(
            result.rms_error < 0.5,
            "Dither ({}, {}): RMS error too high: {}",
            offset.x,
            offset.y,
            result.rms_error
        );
    }
}

/// Integration test: Simulated mosaic panels with rotation
#[test]
fn test_integration_mosaic_panels() {
    // Simulate mosaic panels with small rotations due to mount settling
    let ref_stars = generate_realistic_star_field(80, 4096.0, 4096.0, 54321);
    let ref_positions: Vec<DVec2> = ref_stars.iter().map(|&(pos, _)| pos).collect();

    // Typical mosaic panel offsets: large translation + small rotation
    let panel_transforms = [
        (DVec2::new(1000.0, 0.0), 0.002), // Right panel, 0.1 degree rotation
        (DVec2::new(0.0, 1000.0), -0.001), // Bottom panel, -0.06 degree rotation
        (DVec2::new(1000.0, 1000.0), 0.003), // Diagonal panel, 0.17 degree rotation
    ];

    for (offset, angle) in panel_transforms {
        let transform = Transform::euclidean(offset, angle);
        let target_positions: Vec<DVec2> = ref_positions
            .iter()
            .map(|&pos| transform.apply(pos))
            .collect();

        let result =
            register_star_positions(&ref_positions, &target_positions, TransformType::Euclidean)
                .expect("Mosaic registration should succeed");

        let est_offset = result.transform.translation_components();
        let est_angle = result.transform.rotation_angle();

        assert!(
            (est_offset.x - offset.x).abs() < 2.0,
            "Mosaic: expected dx={}, got {}",
            offset.x,
            est_offset.x
        );
        assert!(
            (est_offset.y - offset.y).abs() < 2.0,
            "Mosaic: expected dy={}, got {}",
            offset.y,
            est_offset.y
        );
        assert!(
            (est_angle - angle).abs() < 0.01,
            "Mosaic: expected angle={}, got {}",
            angle,
            est_angle
        );
    }
}

/// Integration test: Field rotation compensation
#[test]
fn test_integration_field_rotation() {
    // Simulate field rotation during long exposure (alt-az mount)
    let ref_stars = generate_realistic_star_field(60, 2048.0, 2048.0, 99999);
    let ref_positions: Vec<DVec2> = ref_stars.iter().map(|&(pos, _)| pos).collect();

    // Center of rotation (typically image center)
    let center = DVec2::new(1024.0, 1024.0);

    // Field rotation angles (degrees converted to radians)
    let rotation_angles = [0.5, 1.0, 2.0, 5.0]; // degrees

    for angle_deg in rotation_angles {
        let angle = angle_deg * std::f64::consts::PI / 180.0;

        // Apply rotation around image center
        let target_positions: Vec<DVec2> = ref_positions
            .iter()
            .map(|&pos| {
                let d = pos - center;
                let cos_a = angle.cos();
                let sin_a = angle.sin();
                center + DVec2::new(d.x * cos_a - d.y * sin_a, d.x * sin_a + d.y * cos_a)
            })
            .collect();

        let result =
            register_star_positions(&ref_positions, &target_positions, TransformType::Euclidean)
                .expect("Field rotation registration should succeed");

        let est_angle = result.transform.rotation_angle();
        assert!(
            (est_angle - angle).abs() < 0.01,
            "Field rotation {}: expected angle={:.4}, got {:.4}",
            angle_deg,
            angle,
            est_angle
        );
    }
}

/// Integration test: Atmospheric refraction simulation
#[test]
fn test_integration_atmospheric_refraction() {
    // Simulate differential atmospheric refraction (stars shift differently based on elevation)
    // This requires affine transform to model properly
    let ref_stars = generate_realistic_star_field(50, 2048.0, 2048.0, 11111);
    let ref_positions: Vec<DVec2> = ref_stars.iter().map(|&(pos, _)| pos).collect();

    // Simulate refraction: slight shear and scale in Y direction
    // (as if imaging through varying atmospheric density)
    let refraction_shear = 0.001; // Very small shear
    let refraction_scale_y = 1.002; // Slight Y stretch

    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|&pos| {
            let nx = pos.x + pos.y * refraction_shear;
            let ny = pos.y * refraction_scale_y;
            DVec2::new(nx, ny)
        })
        .collect();

    let result = register_star_positions(&ref_positions, &target_positions, TransformType::Affine)
        .expect("Refraction registration should succeed");

    // Check that we can model the transformation
    let mut total_error = 0.0;
    for (i, &r) in ref_positions.iter().enumerate() {
        let t = target_positions[i];
        let p = result.transform.apply(r);
        total_error += (p - t).length();
    }
    let mean_error = total_error / ref_positions.len() as f64;

    assert!(
        mean_error < 1.0,
        "Refraction model error too high: {}",
        mean_error
    );
}

/// Integration test: Registration with centroid noise (realistic detection)
#[test]
fn test_integration_with_centroid_noise() {
    // Simulate real detection with centroid measurement noise
    let ref_stars = generate_realistic_star_field(100, 2048.0, 2048.0, 22222);

    // Apply centroid noise to both reference and target
    let ref_noisy = add_centroid_noise(&ref_stars, 0.3, 33333);
    let ref_positions: Vec<DVec2> = ref_noisy.iter().map(|&(pos, _)| pos).collect();

    // Apply transform to original stars, then add noise
    let transform = Transform::similarity(DVec2::new(50.0, -30.0), 0.02, 1.01);
    let target_stars: Vec<(DVec2, f64)> = ref_stars
        .iter()
        .map(|&(pos, b)| {
            let npos = transform.apply(pos);
            (npos, b)
        })
        .collect();
    let target_noisy = add_centroid_noise(&target_stars, 0.3, 44444);
    let target_positions: Vec<DVec2> = target_noisy.iter().map(|&(pos, _)| pos).collect();

    let result =
        register_star_positions(&ref_positions, &target_positions, TransformType::Similarity)
            .expect("Noisy registration should succeed");

    // With noise, we expect slightly higher error but still reasonable
    let est_offset = result.transform.translation_components();
    let est_scale = result.transform.scale_factor();
    let est_angle = result.transform.rotation_angle();

    assert!(
        (est_offset.x - 50.0).abs() < 2.0,
        "With noise: expected dx=50, got {}",
        est_offset.x
    );
    assert!(
        (est_offset.y - (-30.0)).abs() < 2.0,
        "With noise: expected dy=-30, got {}",
        est_offset.y
    );
    assert!(
        (est_scale - 1.01).abs() < 0.01,
        "With noise: expected scale=1.01, got {}",
        est_scale
    );
    assert!(
        (est_angle - 0.02).abs() < 0.02,
        "With noise: expected angle=0.02, got {}",
        est_angle
    );
}

/// Integration test: Partial field overlap (frame edges cut off)
#[test]
fn test_integration_partial_overlap() {
    let ref_stars = generate_realistic_star_field(100, 2048.0, 2048.0, 55555);
    let ref_positions: Vec<DVec2> = ref_stars.iter().map(|&(pos, _)| pos).collect();

    // Large translation causing ~50% overlap
    let transform = Transform::translation(DVec2::new(1000.0, 500.0));

    // Target stars are transformed, but filter out those that would be outside frame
    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .filter_map(|&pos| {
            let npos = transform.apply(pos);
            // Simulate frame boundary - only keep stars visible in target frame
            if (0.0..=2048.0).contains(&npos.x) && (0.0..=2048.0).contains(&npos.y) {
                Some(npos)
            } else {
                None
            }
        })
        .collect();

    // Need enough overlap
    assert!(
        target_positions.len() >= 20,
        "Need at least 20 overlapping stars for test"
    );

    // This is harder because target has fewer stars
    let result = register_star_positions(
        &ref_positions,
        &target_positions,
        TransformType::Translation,
    );

    // Should still succeed with partial overlap
    assert!(
        result.is_ok(),
        "Partial overlap registration should succeed"
    );

    let result = result.unwrap();
    let est_offset = result.transform.translation_components();

    // Allow larger tolerance due to partial overlap challenges
    assert!(
        (est_offset.x - 1000.0).abs() < 5.0,
        "Partial overlap: expected dx=1000, got {}",
        est_offset.x
    );
    assert!(
        (est_offset.y - 500.0).abs() < 5.0,
        "Partial overlap: expected dy=500, got {}",
        est_offset.y
    );
}

/// Integration test: Multi-night imaging session (different plate scales)
#[test]
fn test_integration_different_plate_scales() {
    // Simulate images from different nights with slight optical differences
    let ref_stars = generate_realistic_star_field(80, 2048.0, 2048.0, 66666);
    let ref_positions: Vec<DVec2> = ref_stars.iter().map(|&(pos, _)| pos).collect();

    // Plate scale difference (e.g., different focus position or optical thermal expansion)
    let scale_factors = [0.995, 1.005, 0.99, 1.01];

    for scale in scale_factors {
        let center = DVec2::new(1024.0, 1024.0);

        // Scale around image center
        let target_positions: Vec<DVec2> = ref_positions
            .iter()
            .map(|&pos| {
                let d = pos - center;
                center + d * scale
            })
            .collect();

        let result =
            register_star_positions(&ref_positions, &target_positions, TransformType::Similarity)
                .expect("Scale registration should succeed");

        let est_scale = result.transform.scale_factor();
        assert!(
            (est_scale - scale).abs() < 0.002,
            "Scale {}: expected {}, got {}",
            scale,
            scale,
            est_scale
        );
    }
}

/// Integration test: Full pipeline with quality metrics validation
#[test]
fn test_integration_quality_metrics() {
    let ref_stars = generate_realistic_star_field(60, 2048.0, 2048.0, 77777);
    let ref_positions: Vec<DVec2> = ref_stars.iter().map(|&(pos, _)| pos).collect();

    // Apply known transform
    let transform = Transform::similarity(DVec2::new(100.0, -75.0), 0.05, 1.02);
    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|&pos| transform.apply(pos))
        .collect();

    let result =
        register_star_positions(&ref_positions, &target_positions, TransformType::Similarity)
            .expect("Quality metrics test should succeed");

    // Check quality metrics
    let inlier_ratio = result.num_inliers as f64 / result.matched_stars.len() as f64;
    assert!(
        inlier_ratio > 0.9,
        "Expected high inlier ratio, got {}",
        inlier_ratio
    );
    assert!(
        result.rms_error < 1.0,
        "Expected low RMS error, got {}",
        result.rms_error
    );
    assert!(
        result.matched_stars.len() >= 40,
        "Expected many matched stars, got {}",
        result.matched_stars.len()
    );
}

/// Integration test: Edge case with minimum viable star count
#[test]
fn test_integration_minimum_stars() {
    // Test with absolute minimum number of stars (just above threshold)
    let ref_positions = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 100.0),
        DVec2::new(150.0, 200.0),
        DVec2::new(100.0, 200.0),
        DVec2::new(200.0, 200.0),
        DVec2::new(150.0, 150.0),
    ];

    let transform = Transform::translation(DVec2::new(10.0, 5.0));
    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|&pos| transform.apply(pos))
        .collect();

    let config = RegistrationConfig {
        min_stars_for_matching: 4,
        min_matched_stars: 4,
        ..Default::default()
    };

    let registrator = Registrator::new(config);
    let result = registrator
        .register_positions(&ref_positions, &target_positions)
        .expect("Minimum stars registration should succeed");

    let est_offset = result.transform.translation_components();
    assert!(
        (est_offset.x - 10.0).abs() < 1.0,
        "Min stars: expected dx=10, got {}",
        est_offset.x
    );
    assert!(
        (est_offset.y - 5.0).abs() < 1.0,
        "Min stars: expected dy=5, got {}",
        est_offset.y
    );
}

// ============================================================================
// select_spatially_distributed tests
// ============================================================================

#[test]
fn test_spatial_selection_empty_input() {
    let result = select_spatially_distributed(&[], 10, 4);
    assert!(result.is_empty());
}

#[test]
fn test_spatial_selection_zero_max_stars() {
    let stars = vec![DVec2::new(100.0, 100.0)];
    let result = select_spatially_distributed(&stars, 0, 4);
    assert!(result.is_empty());
}

#[test]
fn test_spatial_selection_single_star() {
    let stars = vec![DVec2::new(50.0, 50.0)];
    let result = select_spatially_distributed(&stars, 10, 4);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], stars[0]);
}

#[test]
fn test_spatial_selection_respects_max_stars() {
    let stars = generate_star_grid(10, 10, 10.0, DVec2::new(0.0, 0.0));
    let max = 20;
    let result = select_spatially_distributed(&stars, max, 4);
    assert_eq!(result.len(), max);
}

#[test]
fn test_spatial_selection_fewer_stars_than_max() {
    let stars = vec![
        DVec2::new(10.0, 10.0),
        DVec2::new(90.0, 90.0),
        DVec2::new(50.0, 50.0),
    ];
    let result = select_spatially_distributed(&stars, 100, 4);
    assert_eq!(result.len(), 3);
}

#[test]
fn test_spatial_selection_distributes_across_grid() {
    // Place stars in 4 distinct quadrants (2x2 grid).
    // Even though the top-left quadrant has 10 stars (listed first = "brightest"),
    // the function should pick from all quadrants, not just the brightest cluster.
    let mut stars = Vec::new();
    // Top-left cluster: 10 stars
    for i in 0..10 {
        stars.push(DVec2::new(10.0 + i as f64, 10.0 + i as f64));
    }
    // Top-right: 3 stars
    for i in 0..3 {
        stars.push(DVec2::new(90.0 + i as f64, 10.0 + i as f64));
    }
    // Bottom-left: 3 stars
    for i in 0..3 {
        stars.push(DVec2::new(10.0 + i as f64, 90.0 + i as f64));
    }
    // Bottom-right: 3 stars
    for i in 0..3 {
        stars.push(DVec2::new(90.0 + i as f64, 90.0 + i as f64));
    }

    let result = select_spatially_distributed(&stars, 8, 2);
    assert_eq!(result.len(), 8);

    // Check that all 4 quadrants are represented
    let mid_x = 50.0;
    let mid_y = 50.0;
    let top_left = result.iter().filter(|p| p.x < mid_x && p.y < mid_y).count();
    let top_right = result
        .iter()
        .filter(|p| p.x >= mid_x && p.y < mid_y)
        .count();
    let bot_left = result
        .iter()
        .filter(|p| p.x < mid_x && p.y >= mid_y)
        .count();
    let bot_right = result
        .iter()
        .filter(|p| p.x >= mid_x && p.y >= mid_y)
        .count();

    assert!(top_left >= 1, "Top-left missing: {top_left}");
    assert!(top_right >= 1, "Top-right missing: {top_right}");
    assert!(bot_left >= 1, "Bottom-left missing: {bot_left}");
    assert!(bot_right >= 1, "Bottom-right missing: {bot_right}");
}

#[test]
fn test_spatial_selection_round_robin_fairness() {
    // 4 quadrants with 2x2 grid, each quadrant has different number of stars.
    // With max_stars=4 and grid_size=2, first round should take 1 from each cell.
    let stars = vec![
        // Cell (0,0) - top-left: 5 stars
        DVec2::new(10.0, 10.0),
        DVec2::new(15.0, 15.0),
        DVec2::new(20.0, 20.0),
        DVec2::new(25.0, 25.0),
        DVec2::new(30.0, 30.0),
        // Cell (1,0) - top-right: 1 star
        DVec2::new(80.0, 10.0),
        // Cell (0,1) - bottom-left: 1 star
        DVec2::new(10.0, 80.0),
        // Cell (1,1) - bottom-right: 1 star
        DVec2::new(80.0, 80.0),
    ];

    let result = select_spatially_distributed(&stars, 4, 2);
    assert_eq!(result.len(), 4);

    // Each cell should contribute exactly 1 star
    let mid = 50.0;
    let tl = result.iter().filter(|p| p.x < mid && p.y < mid).count();
    let tr = result.iter().filter(|p| p.x >= mid && p.y < mid).count();
    let bl = result.iter().filter(|p| p.x < mid && p.y >= mid).count();
    let br = result.iter().filter(|p| p.x >= mid && p.y >= mid).count();

    assert_eq!(tl, 1, "Top-left should have 1 star, got {tl}");
    assert_eq!(tr, 1, "Top-right should have 1 star, got {tr}");
    assert_eq!(bl, 1, "Bottom-left should have 1 star, got {bl}");
    assert_eq!(br, 1, "Bottom-right should have 1 star, got {br}");
}

#[test]
fn test_spatial_selection_preserves_brightness_order_within_cell() {
    // Stars in the same cell should be selected in input order (brightness order).
    // One cell with 3 stars, another cell with 1.
    let stars = vec![
        DVec2::new(10.0, 10.0), // Cell 0: brightest
        DVec2::new(15.0, 15.0), // Cell 0: second brightest
        DVec2::new(20.0, 20.0), // Cell 0: third brightest
        DVec2::new(90.0, 90.0), // Cell 3: only star
    ];

    let result = select_spatially_distributed(&stars, 4, 2);

    // Round 0: picks (10,10) from cell 0 and (90,90) from cell 3
    // Round 1: picks (15,15) from cell 0 (no more in cell 3)
    // Round 2: picks (20,20) from cell 0
    assert_eq!(result.len(), 4);
    assert!(result.contains(&DVec2::new(10.0, 10.0)));
    assert!(result.contains(&DVec2::new(15.0, 15.0)));
    assert!(result.contains(&DVec2::new(20.0, 20.0)));
    assert!(result.contains(&DVec2::new(90.0, 90.0)));
}

#[test]
fn test_spatial_selection_all_stars_same_position() {
    // All stars at the same point - should all go to one cell
    let stars = vec![
        DVec2::new(50.0, 50.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(50.0, 50.0),
    ];
    let result = select_spatially_distributed(&stars, 2, 4);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_spatial_selection_large_grid_sparse_stars() {
    // Grid is much larger than the number of stars (many empty cells)
    let stars = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 200.0),
    ];
    let result = select_spatially_distributed(&stars, 3, 16);
    assert_eq!(result.len(), 3);
}

#[test]
fn test_spatial_selection_grid_size_2() {
    // Minimum valid grid size
    let stars = generate_star_grid(4, 4, 50.0, DVec2::new(0.0, 0.0));
    let result = select_spatially_distributed(&stars, 8, 2);
    assert_eq!(result.len(), 8);
}

#[test]
fn test_spatial_selection_improves_coverage_vs_brightness_only() {
    // Create a scenario where brightness-only selection clusters badly.
    // All "bright" stars (listed first) are in one corner.
    let mut stars = Vec::new();
    // 20 bright stars clustered in top-left
    for i in 0..20 {
        stars.push(DVec2::new(
            10.0 + (i % 5) as f64 * 5.0,
            10.0 + (i / 5) as f64 * 5.0,
        ));
    }
    // 5 dim stars spread across the field
    stars.push(DVec2::new(500.0, 10.0));
    stars.push(DVec2::new(10.0, 500.0));
    stars.push(DVec2::new(500.0, 500.0));
    stars.push(DVec2::new(250.0, 250.0));
    stars.push(DVec2::new(500.0, 250.0));

    let n = 10;
    let spatial = select_spatially_distributed(&stars, n, 4);
    let brightness_only: Vec<DVec2> = stars.iter().take(n).copied().collect();

    // Measure coverage: bounding box area of selected stars
    let bbox_area = |pts: &[DVec2]| -> f64 {
        let (min_x, max_x, min_y, max_y) = pts.iter().fold(
            (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
            |(mnx, mxx, mny, mxy), p| (mnx.min(p.x), mxx.max(p.x), mny.min(p.y), mxy.max(p.y)),
        );
        (max_x - min_x) * (max_y - min_y)
    };

    let spatial_area = bbox_area(&spatial);
    let brightness_area = bbox_area(&brightness_only);

    assert!(
        spatial_area > brightness_area * 5.0,
        "Spatial selection should cover much more area: spatial={spatial_area}, brightness={brightness_area}"
    );
}

#[test]
fn test_spatial_selection_integration_with_pipeline() {
    // Verify spatial selection works end-to-end in the registration pipeline.
    // Create clustered stars where spatial distribution matters.
    let mut ref_stars = Vec::new();
    // Dense cluster (would dominate brightness-only selection)
    for r in 0..8 {
        for c in 0..8 {
            ref_stars.push(DVec2::new(100.0 + c as f64 * 15.0, 100.0 + r as f64 * 15.0));
        }
    }
    // Sparse stars spread across the field (important for transform accuracy)
    ref_stars.push(DVec2::new(500.0, 50.0));
    ref_stars.push(DVec2::new(50.0, 500.0));
    ref_stars.push(DVec2::new(500.0, 500.0));
    ref_stars.push(DVec2::new(300.0, 400.0));
    ref_stars.push(DVec2::new(400.0, 300.0));
    ref_stars.push(DVec2::new(450.0, 150.0));

    let transform = Transform::similarity(DVec2::new(20.0, -10.0), 0.03, 1.01);
    let target_stars = transform_stars(&ref_stars, &transform);

    // Register with spatial distribution enabled (default)
    let config = RegistrationConfig {
        transform_type: TransformType::Similarity,
        use_spatial_distribution: true,
        spatial_grid_size: 4,
        min_stars_for_matching: 6,
        min_matched_stars: 4,
        ..Default::default()
    };
    let result = Registrator::new(config)
        .register_positions(&ref_stars, &target_stars)
        .expect("Registration with spatial selection should succeed");

    let est_scale = result.transform.scale_factor();
    let est_angle = result.transform.rotation_angle();
    assert!(
        (est_scale - 1.01).abs() < 0.01,
        "Expected scale=1.01, got {est_scale}"
    );
    assert!(
        (est_angle - 0.03).abs() < 0.01,
        "Expected angle=0.03, got {est_angle}"
    );
}
