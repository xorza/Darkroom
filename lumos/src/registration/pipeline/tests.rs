use super::*;
use crate::AstroImage;
use crate::common::Buffer2;
use crate::registration::transform::TransformType;
use std::f64::consts::PI;

/// Test helper: register star positions with specified transform type
fn register_star_positions(
    ref_positions: &[(f64, f64)],
    target_positions: &[(f64, f64)],
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

fn generate_star_grid(
    rows: usize,
    cols: usize,
    spacing: f64,
    offset: (f64, f64),
) -> Vec<(f64, f64)> {
    let mut stars = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let x = offset.0 + c as f64 * spacing;
            let y = offset.1 + r as f64 * spacing;
            stars.push((x, y));
        }
    }
    stars
}

fn transform_stars(stars: &[(f64, f64)], transform: &Transform) -> Vec<(f64, f64)> {
    stars.iter().map(|&(x, y)| transform.apply(x, y)).collect()
}

#[test]
fn test_registration_identity() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (100.0, 100.0));
    let target_stars = ref_stars.clone();

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    // Should find near-identity transform
    let (tx, ty) = result.transform.translation_components();
    assert!(tx.abs() < 1.0, "Expected near-zero translation, got {}", tx);
    assert!(ty.abs() < 1.0, "Expected near-zero translation, got {}", ty);
    assert!(result.rms_error < 0.5, "Expected low RMS error");
}

#[test]
fn test_registration_translation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (100.0, 100.0));
    let translation = Transform::translation(50.0, -30.0);
    let target_stars = transform_stars(&ref_stars, &translation);

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let (tx, ty) = result.transform.translation_components();
    assert!((tx - 50.0).abs() < 1.0, "Expected tx=50, got {}", tx);
    assert!((ty - (-30.0)).abs() < 1.0, "Expected ty=-30, got {}", ty);
}

#[test]
fn test_registration_rotation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (200.0, 200.0));
    let rotation = Transform::euclidean(10.0, -5.0, 0.1); // ~5.7 degrees
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
    let ref_stars = generate_star_grid(5, 5, 100.0, (200.0, 200.0));
    let similarity = Transform::similarity(20.0, 15.0, 0.05, 1.02);
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
    let ref_stars = generate_star_grid(6, 6, 80.0, (100.0, 100.0));
    let translation = Transform::translation(25.0, 40.0);
    let mut target_stars = transform_stars(&ref_stars, &translation);

    // Add outliers (wrong matches)
    target_stars[0] = (500.0, 500.0);
    target_stars[5] = (50.0, 800.0);
    target_stars[10] = (900.0, 100.0);

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let (tx, ty) = result.transform.translation_components();
    // RANSAC should still find correct translation despite outliers
    assert!((tx - 25.0).abs() < 2.0, "Expected tx=25, got {}", tx);
    assert!((ty - 40.0).abs() < 2.0, "Expected ty=40, got {}", ty);
}

#[test]
fn test_registration_insufficient_stars() {
    let ref_stars = vec![(100.0, 100.0), (200.0, 200.0)];
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
        ransac_iterations: 2000,
        ransac_threshold: 1.5,
        ..Default::default()
    };

    let registrator = Registrator::new(config);
    assert_eq!(registrator.config().ransac_iterations, 2000);
    assert!((registrator.config().ransac_threshold - 1.5).abs() < 1e-10);
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
    let transform = Transform::translation(5.0, 3.0);

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
    let transform = Transform::similarity(10.0, -5.0, 0.1, 1.02);

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
    let ref_stars = generate_star_grid(6, 6, 35.0, (30.0, 30.0));

    // Create reference image with Gaussian stars
    let ref_pixels = generate_synthetic_star_image(width, height, &ref_stars, 1.0, 4.0);

    // Apply known transform to star positions
    let known_transform = Transform::similarity(12.0, -8.0, 0.05, 1.01);
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
    let ref_stars = generate_star_grid(4, 4, 150.0, (100.0, 100.0));
    let translation = Transform::translation(10.0, -15.0);
    let target_stars = transform_stars(&ref_stars, &translation);

    let transform = quick_register(&ref_stars, &target_stars).unwrap();
    let (tx, ty) = transform.translation_components();

    assert!((tx - 10.0).abs() < 1.0);
    assert!((ty - (-15.0)).abs() < 1.0);
}

#[test]
fn test_registration_result_quality() {
    let ref_stars = generate_star_grid(6, 6, 100.0, (50.0, 50.0));
    let target_stars = ref_stars.clone();

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    // Perfect match should have very low error and high quality
    assert!(result.rms_error < 0.1);
    assert!(result.num_inliers >= 20);
}

#[test]
fn test_registration_large_rotation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (250.0, 250.0));
    // 30 degree rotation around image center
    let rotation = Transform::rotation_around(300.0, 300.0, PI / 6.0);
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
    stars: &[(f64, f64)],
    brightness: f32,
    fwhm: f32,
) -> Vec<f32> {
    let mut image = vec![0.0f32; width * height];
    let sigma = fwhm / 2.355; // FWHM to sigma conversion

    for &(sx, sy) in stars {
        // Draw Gaussian profile around each star
        let radius = (3.0 * sigma) as i32;
        let cx = sx as i32;
        let cy = sy as i32;

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
    let ref_stars = generate_star_grid(6, 6, 35.0, (30.0, 30.0));

    // Apply known transform
    let known_transform = Transform::similarity(15.0, -10.0, 0.05, 1.02);
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
    let (_est_tx, _est_ty) = result.transform.translation_components();

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
    let ref_stars = generate_star_grid(6, 6, 50.0, (50.0, 50.0));
    let large_translation = Transform::translation(150.0, 0.0);
    let target_stars = transform_stars(&ref_stars, &large_translation);

    let result =
        register_star_positions(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let (tx, ty) = result.transform.translation_components();
    assert!(
        (tx - 150.0).abs() < 2.0,
        "Large translation error: expected 150, got {}",
        tx
    );
    assert!(ty.abs() < 2.0, "Unexpected y translation: {}", ty);

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
    let ref_stars = generate_star_grid(6, 6, 80.0, (100.0, 100.0));
    let transform = Transform::similarity(10.0, -5.0, 0.03, 1.01);

    // Add noise to target positions
    let mut target_stars = transform_stars(&ref_stars, &transform);
    for (i, star) in target_stars.iter_mut().enumerate() {
        // Deterministic "noise" based on index
        let noise_x = ((i * 7) as f64 * 0.1).sin() * 0.5;
        let noise_y = ((i * 11) as f64 * 0.1).cos() * 0.5;
        star.0 += noise_x;
        star.1 += noise_y;
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
    let ref_stars = generate_star_grid(5, 5, 100.0, (100.0, 100.0));
    let transform = Transform::similarity(20.0, 15.0, 0.08, 1.03);
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
    let ref_stars = generate_star_grid(6, 6, 80.0, (100.0, 100.0));

    // Affine with slight shear
    let affine = Transform::affine([1.02, 0.03, 15.0, -0.02, 0.98, 10.0]);
    let target_stars = transform_stars(&ref_stars, &affine);

    let result = register_star_positions(&ref_stars, &target_stars, TransformType::Affine).unwrap();

    // Verify by transforming reference points and checking error
    let mut total_error = 0.0;
    for (i, &(rx, ry)) in ref_stars.iter().enumerate() {
        let (tx, ty) = target_stars[i];
        let (px, py) = result.transform.apply(rx, ry);
        total_error += ((px - tx).powi(2) + (py - ty).powi(2)).sqrt();
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
    let ref_stars = generate_star_grid(10, 10, 50.0, (50.0, 50.0));
    let translation = Transform::translation(25.0, -15.0);
    let target_stars = transform_stars(&ref_stars, &translation);

    let config = RegistrationConfig {
        min_stars_for_matching: 6,
        min_matched_stars: 4,
        max_residual_pixels: 5.0,
        ..Default::default()
    };

    let multiscale_config = MultiScaleConfig {
        levels: 2,
        scale_factor: 2.0,
        min_dimension: 64,
        use_phase_correlation: false,
    };

    let registrator = MultiScaleRegistrator::new(config, multiscale_config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars, 1000, 1000)
        .unwrap();

    let (tx, ty) = result.transform.translation_components();
    assert!((tx - 25.0).abs() < 2.0, "Expected tx=25, got {}", tx);
    assert!((ty - (-15.0)).abs() < 2.0, "Expected ty=-15, got {}", ty);
}

#[test]
fn test_multiscale_registration_with_rotation() {
    let ref_stars = generate_star_grid(8, 8, 60.0, (100.0, 100.0));

    // Apply rotation + translation
    let angle = PI / 20.0; // 9 degrees
    let transform = Transform::similarity(30.0, -20.0, angle, 1.0);
    let target_stars = transform_stars(&ref_stars, &transform);

    let config = RegistrationConfig {
        transform_type: TransformType::Similarity,
        min_stars_for_matching: 6,
        min_matched_stars: 4,
        max_residual_pixels: 5.0,
        ..Default::default()
    };

    let multiscale_config = MultiScaleConfig {
        levels: 2,
        scale_factor: 2.0,
        min_dimension: 64,
        use_phase_correlation: false,
    };

    let registrator = MultiScaleRegistrator::new(config, multiscale_config);
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
    let transform = Transform::translation(10.0, 20.0);
    let scaled = super::scale_transform(&transform, 2.0);

    let (tx, ty) = scaled.translation_components();
    assert!((tx - 20.0).abs() < 0.01);
    assert!((ty - 40.0).abs() < 0.01);
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
) -> Vec<(f64, f64, f64)> {
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
        stars.push((x, y, brightness));
    }

    // Sort by brightness (brightest first, as detection would)
    stars.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    stars
}

/// Simulate star detection noise (centroid uncertainty)
fn add_centroid_noise(
    stars: &[(f64, f64, f64)],
    noise_sigma: f64,
    seed: u64,
) -> Vec<(f64, f64, f64)> {
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
        .map(|&(x, y, b)| {
            // Fainter stars have more centroid uncertainty
            let actual_sigma = noise_sigma / b.sqrt();
            let nx = x + gaussian() * actual_sigma;
            let ny = y + gaussian() * actual_sigma;
            (nx, ny, b)
        })
        .collect()
}

/// Integration test: Simulated dithered exposure sequence
#[test]
fn test_integration_dithered_exposures() {
    // Simulate a typical dithering pattern with small offsets
    let ref_stars = generate_realistic_star_field(100, 2048.0, 2048.0, 12345);
    let ref_positions: Vec<(f64, f64)> = ref_stars.iter().map(|&(x, y, _)| (x, y)).collect();

    // Dithering offsets (typical 5-50 pixel offsets)
    let dither_offsets = [(10.0, 15.0), (-20.0, 5.0), (8.0, -12.0), (-5.0, 25.0)];

    for (dx, dy) in dither_offsets {
        let transform = Transform::translation(dx, dy);
        let target_positions: Vec<(f64, f64)> = ref_positions
            .iter()
            .map(|&(x, y)| transform.apply(x, y))
            .collect();

        let result = register_star_positions(
            &ref_positions,
            &target_positions,
            TransformType::Translation,
        )
        .expect("Dithered registration should succeed");

        let (est_dx, est_dy) = result.transform.translation_components();
        assert!(
            (est_dx - dx).abs() < 0.5,
            "Dither ({}, {}): expected dx={}, got {}",
            dx,
            dy,
            dx,
            est_dx
        );
        assert!(
            (est_dy - dy).abs() < 0.5,
            "Dither ({}, {}): expected dy={}, got {}",
            dx,
            dy,
            dy,
            est_dy
        );
        assert!(
            result.rms_error < 0.5,
            "Dither ({}, {}): RMS error too high: {}",
            dx,
            dy,
            result.rms_error
        );
    }
}

/// Integration test: Simulated mosaic panels with rotation
#[test]
fn test_integration_mosaic_panels() {
    // Simulate mosaic panels with small rotations due to mount settling
    let ref_stars = generate_realistic_star_field(80, 4096.0, 4096.0, 54321);
    let ref_positions: Vec<(f64, f64)> = ref_stars.iter().map(|&(x, y, _)| (x, y)).collect();

    // Typical mosaic panel offsets: large translation + small rotation
    let panel_transforms = [
        (1000.0, 0.0, 0.002),    // Right panel, 0.1 degree rotation
        (0.0, 1000.0, -0.001),   // Bottom panel, -0.06 degree rotation
        (1000.0, 1000.0, 0.003), // Diagonal panel, 0.17 degree rotation
    ];

    for (dx, dy, angle) in panel_transforms {
        let transform = Transform::euclidean(dx, dy, angle);
        let target_positions: Vec<(f64, f64)> = ref_positions
            .iter()
            .map(|&(x, y)| transform.apply(x, y))
            .collect();

        let result =
            register_star_positions(&ref_positions, &target_positions, TransformType::Euclidean)
                .expect("Mosaic registration should succeed");

        let (est_dx, est_dy) = result.transform.translation_components();
        let est_angle = result.transform.rotation_angle();

        assert!(
            (est_dx - dx).abs() < 2.0,
            "Mosaic: expected dx={}, got {}",
            dx,
            est_dx
        );
        assert!(
            (est_dy - dy).abs() < 2.0,
            "Mosaic: expected dy={}, got {}",
            dy,
            est_dy
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
    let ref_positions: Vec<(f64, f64)> = ref_stars.iter().map(|&(x, y, _)| (x, y)).collect();

    // Center of rotation (typically image center)
    let cx = 1024.0;
    let cy = 1024.0;

    // Field rotation angles (degrees converted to radians)
    let rotation_angles = [0.5, 1.0, 2.0, 5.0]; // degrees

    for angle_deg in rotation_angles {
        let angle = angle_deg * std::f64::consts::PI / 180.0;

        // Apply rotation around image center
        let target_positions: Vec<(f64, f64)> = ref_positions
            .iter()
            .map(|&(x, y)| {
                let dx = x - cx;
                let dy = y - cy;
                let cos_a = angle.cos();
                let sin_a = angle.sin();
                (cx + dx * cos_a - dy * sin_a, cy + dx * sin_a + dy * cos_a)
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
    let ref_positions: Vec<(f64, f64)> = ref_stars.iter().map(|&(x, y, _)| (x, y)).collect();

    // Simulate refraction: slight shear and scale in Y direction
    // (as if imaging through varying atmospheric density)
    let refraction_shear = 0.001; // Very small shear
    let refraction_scale_y = 1.002; // Slight Y stretch

    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|&(x, y)| {
            let nx = x + y * refraction_shear;
            let ny = y * refraction_scale_y;
            (nx, ny)
        })
        .collect();

    let result = register_star_positions(&ref_positions, &target_positions, TransformType::Affine)
        .expect("Refraction registration should succeed");

    // Check that we can model the transformation
    let mut total_error = 0.0;
    for (i, &(rx, ry)) in ref_positions.iter().enumerate() {
        let (tx, ty) = target_positions[i];
        let (px, py) = result.transform.apply(rx, ry);
        total_error += ((px - tx).powi(2) + (py - ty).powi(2)).sqrt();
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
    let ref_positions: Vec<(f64, f64)> = ref_noisy.iter().map(|&(x, y, _)| (x, y)).collect();

    // Apply transform to original stars, then add noise
    let transform = Transform::similarity(50.0, -30.0, 0.02, 1.01);
    let target_stars: Vec<(f64, f64, f64)> = ref_stars
        .iter()
        .map(|&(x, y, b)| {
            let (nx, ny) = transform.apply(x, y);
            (nx, ny, b)
        })
        .collect();
    let target_noisy = add_centroid_noise(&target_stars, 0.3, 44444);
    let target_positions: Vec<(f64, f64)> = target_noisy.iter().map(|&(x, y, _)| (x, y)).collect();

    let result =
        register_star_positions(&ref_positions, &target_positions, TransformType::Similarity)
            .expect("Noisy registration should succeed");

    // With noise, we expect slightly higher error but still reasonable
    let (est_dx, est_dy) = result.transform.translation_components();
    let est_scale = result.transform.scale_factor();
    let est_angle = result.transform.rotation_angle();

    assert!(
        (est_dx - 50.0).abs() < 2.0,
        "With noise: expected dx=50, got {}",
        est_dx
    );
    assert!(
        (est_dy - (-30.0)).abs() < 2.0,
        "With noise: expected dy=-30, got {}",
        est_dy
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
    let ref_positions: Vec<(f64, f64)> = ref_stars.iter().map(|&(x, y, _)| (x, y)).collect();

    // Large translation causing ~50% overlap
    let transform = Transform::translation(1000.0, 500.0);

    // Target stars are transformed, but filter out those that would be outside frame
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .filter_map(|&(x, y)| {
            let (nx, ny) = transform.apply(x, y);
            // Simulate frame boundary - only keep stars visible in target frame
            if (0.0..=2048.0).contains(&nx) && (0.0..=2048.0).contains(&ny) {
                Some((nx, ny))
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
    let (est_dx, est_dy) = result.transform.translation_components();

    // Allow larger tolerance due to partial overlap challenges
    assert!(
        (est_dx - 1000.0).abs() < 5.0,
        "Partial overlap: expected dx=1000, got {}",
        est_dx
    );
    assert!(
        (est_dy - 500.0).abs() < 5.0,
        "Partial overlap: expected dy=500, got {}",
        est_dy
    );
}

/// Integration test: Multi-night imaging session (different plate scales)
#[test]
fn test_integration_different_plate_scales() {
    // Simulate images from different nights with slight optical differences
    let ref_stars = generate_realistic_star_field(80, 2048.0, 2048.0, 66666);
    let ref_positions: Vec<(f64, f64)> = ref_stars.iter().map(|&(x, y, _)| (x, y)).collect();

    // Plate scale difference (e.g., different focus position or optical thermal expansion)
    let scale_factors = [0.995, 1.005, 0.99, 1.01];

    for scale in scale_factors {
        let cx = 1024.0;
        let cy = 1024.0;

        // Scale around image center
        let target_positions: Vec<(f64, f64)> = ref_positions
            .iter()
            .map(|&(x, y)| {
                let dx = x - cx;
                let dy = y - cy;
                (cx + dx * scale, cy + dy * scale)
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
    let ref_positions: Vec<(f64, f64)> = ref_stars.iter().map(|&(x, y, _)| (x, y)).collect();

    // Apply known transform
    let transform = Transform::similarity(100.0, -75.0, 0.05, 1.02);
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|&(x, y)| transform.apply(x, y))
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
        (100.0, 100.0),
        (200.0, 100.0),
        (150.0, 200.0),
        (100.0, 200.0),
        (200.0, 200.0),
        (150.0, 150.0),
    ];

    let transform = Transform::translation(10.0, 5.0);
    let target_positions: Vec<(f64, f64)> = ref_positions
        .iter()
        .map(|&(x, y)| transform.apply(x, y))
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

    let (est_dx, est_dy) = result.transform.translation_components();
    assert!(
        (est_dx - 10.0).abs() < 1.0,
        "Min stars: expected dx=10, got {}",
        est_dx
    );
    assert!(
        (est_dy - 5.0).abs() < 1.0,
        "Min stars: expected dy=5, got {}",
        est_dy
    );
}
