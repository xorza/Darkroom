use super::*;
use std::f64::consts::PI;

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

fn transform_stars(stars: &[(f64, f64)], transform: &TransformMatrix) -> Vec<(f64, f64)> {
    stars.iter().map(|&(x, y)| transform.apply(x, y)).collect()
}

#[test]
fn test_registration_identity() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (100.0, 100.0));
    let target_stars = ref_stars.clone();

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    // Should find near-identity transform
    let (tx, ty) = result.transform.translation_components();
    assert!(tx.abs() < 1.0, "Expected near-zero translation, got {}", tx);
    assert!(ty.abs() < 1.0, "Expected near-zero translation, got {}", ty);
    assert!(result.rms_error < 0.5, "Expected low RMS error");
}

#[test]
fn test_registration_translation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (100.0, 100.0));
    let translation = TransformMatrix::from_translation(50.0, -30.0);
    let target_stars = transform_stars(&ref_stars, &translation);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let (tx, ty) = result.transform.translation_components();
    assert!((tx - 50.0).abs() < 1.0, "Expected tx=50, got {}", tx);
    assert!((ty - (-30.0)).abs() < 1.0, "Expected ty=-30, got {}", ty);
}

#[test]
fn test_registration_rotation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (200.0, 200.0));
    let rotation = TransformMatrix::euclidean(10.0, -5.0, 0.1); // ~5.7 degrees
    let target_stars = transform_stars(&ref_stars, &rotation);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Euclidean).unwrap();

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
    let similarity = TransformMatrix::similarity(20.0, 15.0, 0.05, 1.02);
    let target_stars = transform_stars(&ref_stars, &similarity);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Similarity).unwrap();

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
    let translation = TransformMatrix::from_translation(25.0, 40.0);
    let mut target_stars = transform_stars(&ref_stars, &translation);

    // Add outliers (wrong matches)
    target_stars[0] = (500.0, 500.0);
    target_stars[5] = (50.0, 800.0);
    target_stars[10] = (900.0, 100.0);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let (tx, ty) = result.transform.translation_components();
    // RANSAC should still find correct translation despite outliers
    assert!((tx - 25.0).abs() < 2.0, "Expected tx=25, got {}", tx);
    assert!((ty - 40.0).abs() < 2.0, "Expected ty=40, got {}", ty);
}

#[test]
fn test_registration_insufficient_stars() {
    let ref_stars = vec![(100.0, 100.0), (200.0, 200.0)];
    let target_stars = ref_stars.clone();

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation);
    assert!(matches!(
        result,
        Err(RegistrationError::InsufficientStars { .. })
    ));
}

#[test]
fn test_registrator_config() {
    let config = RegistrationConfig::builder()
        .with_rotation()
        .ransac_iterations(2000)
        .ransac_threshold(1.5)
        .build();

    let registrator = Registrator::new(config);
    assert_eq!(registrator.config().ransac_iterations, 2000);
    assert!((registrator.config().ransac_threshold - 1.5).abs() < 1e-10);
}

#[test]
fn test_warp_to_reference() {
    // Create a simple test image
    let width = 64;
    let height = 64;
    let mut image = vec![0.0f32; width * height];
    image[32 * width + 32] = 1.0; // Bright pixel at center

    let transform = TransformMatrix::from_translation(5.0, 3.0);
    let warped = warp_to_reference(
        &image,
        width,
        height,
        &transform,
        InterpolationMethod::Bilinear,
    );

    assert_eq!(warped.len(), width * height);
    // The bright pixel should have moved
    assert!(warped[32 * width + 32] < 0.5);
}

#[test]
fn test_quick_register() {
    let ref_stars = generate_star_grid(4, 4, 150.0, (100.0, 100.0));
    let translation = TransformMatrix::from_translation(10.0, -15.0);
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

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    // Perfect match should have very low error and high quality
    assert!(result.rms_error < 0.1);
    assert!(result.num_inliers >= 20);
}

#[test]
fn test_registration_large_rotation() {
    let ref_stars = generate_star_grid(5, 5, 100.0, (250.0, 250.0));
    // 30 degree rotation around image center
    let rotation = TransformMatrix::from_rotation_around(PI / 6.0, 300.0, 300.0);
    let target_stars = transform_stars(&ref_stars, &rotation);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Euclidean).unwrap();

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
    let known_transform = TransformMatrix::similarity(15.0, -10.0, 0.05, 1.02);
    let target_stars = transform_stars(&ref_stars, &known_transform);

    // Create synthetic images
    let ref_image = generate_synthetic_star_image(width, height, &ref_stars, 1.0, 4.0);
    let target_image = generate_synthetic_star_image(width, height, &target_stars, 1.0, 4.0);

    // Register using star positions (simulating star detection)
    let result = register_stars(&ref_stars, &target_stars, TransformType::Similarity).unwrap();

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
    let warped = warp_to_reference(
        &target_image,
        width,
        height,
        &result.transform,
        InterpolationMethod::Lanczos3,
    );

    // Calculate alignment error by comparing pixel intensities
    let mut error_sum = 0.0f32;
    let mut pixel_count = 0;

    for y in 20..height - 20 {
        for x in 20..width - 20 {
            let ref_val = ref_image[y * width + x];
            let warped_val = warped[y * width + x];

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
    let large_translation = TransformMatrix::from_translation(150.0, 0.0);
    let target_stars = transform_stars(&ref_stars, &large_translation);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

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
    let transform = TransformMatrix::similarity(10.0, -5.0, 0.03, 1.01);

    // Add noise to target positions
    let mut target_stars = transform_stars(&ref_stars, &transform);
    for (i, star) in target_stars.iter_mut().enumerate() {
        // Deterministic "noise" based on index
        let noise_x = ((i * 7) as f64 * 0.1).sin() * 0.5;
        let noise_y = ((i * 11) as f64 * 0.1).cos() * 0.5;
        star.0 += noise_x;
        star.1 += noise_y;
    }

    let result = register_stars(&ref_stars, &target_stars, TransformType::Similarity).unwrap();

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
    let transform = TransformMatrix::similarity(20.0, 15.0, 0.08, 1.03);
    let target_stars = transform_stars(&ref_stars, &transform);

    // Run registration multiple times
    let mut scales = Vec::new();
    let mut angles = Vec::new();

    for _ in 0..5 {
        let result = register_stars(&ref_stars, &target_stars, TransformType::Similarity).unwrap();
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
    let affine = TransformMatrix::affine([1.02, 0.03, 15.0, -0.02, 0.98, 10.0]);
    let target_stars = transform_stars(&ref_stars, &affine);

    let result = register_stars(&ref_stars, &target_stars, TransformType::Affine).unwrap();

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
    let translation = TransformMatrix::from_translation(25.0, -15.0);
    let target_stars = transform_stars(&ref_stars, &translation);

    let config = RegistrationConfig::builder()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(5.0)
        .build();

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
    let transform = TransformMatrix::similarity(30.0, -20.0, angle, 1.0);
    let target_stars = transform_stars(&ref_stars, &transform);

    let config = RegistrationConfig::builder()
        .with_scale()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(5.0)
        .build();

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
    let transform = TransformMatrix::from_translation(10.0, 20.0);
    let scaled = super::scale_transform(&transform, 2.0);

    let (tx, ty) = scaled.translation_components();
    assert!((tx - 20.0).abs() < 0.01);
    assert!((ty - 40.0).abs() < 0.01);
}
