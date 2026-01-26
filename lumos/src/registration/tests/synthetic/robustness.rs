//! Robustness tests for registration.
//!
//! Tests for:
//! - Outlier rejection (spurious/missing stars)
//! - Partial overlap
//! - Subpixel accuracy
//! - Minimum star counts
//! - Combined disturbances (stress tests)

use crate::registration::{RegistrationConfig, Registrator};
use crate::testing::synthetic::{
    add_position_noise, add_spurious_stars, generate_random_positions, remove_random_stars,
    transform_stars, translate_stars, translate_with_overlap,
};

// ============================================================================
// Outlier Rejection Tests
// ============================================================================

#[test]
fn test_outlier_rejection_spurious_stars() {
    // 10% spurious stars in target (false detections)
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 11111);

    let dx = 30.0;
    let dy = -20.0;
    let target_stars = translate_stars(&ref_stars, dx, dy);

    // Add 10 spurious stars (10% of original)
    let target_with_spurious = add_spurious_stars(&target_stars, 10, 2000.0, 2000.0, 22222);

    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_with_spurious)
        .expect("Registration should succeed despite spurious stars");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    assert!(
        (recovered_dx - dx).abs() < 1.0,
        "X translation error too large with spurious stars: expected {}, got {}",
        dx,
        recovered_dx
    );
    assert!(
        (recovered_dy - dy).abs() < 1.0,
        "Y translation error too large with spurious stars: expected {}, got {}",
        dy,
        recovered_dy
    );
    assert!(
        result.rms_error < 2.0,
        "RMS error too large: {}",
        result.rms_error
    );
}

#[test]
fn test_outlier_rejection_missing_stars() {
    // 10% missing stars in target (undetected real stars)
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 33333);

    let dx = 25.0;
    let dy = 15.0;
    let target_stars = translate_stars(&ref_stars, dx, dy);

    // Remove ~10% of stars
    let target_with_missing = remove_random_stars(&target_stars, 0.1, 44444);

    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_with_missing)
        .expect("Registration should succeed despite missing stars");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    assert!(
        (recovered_dx - dx).abs() < 1.0,
        "X translation error too large with missing stars: expected {}, got {}",
        dx,
        recovered_dx
    );
    assert!(
        (recovered_dy - dy).abs() < 1.0,
        "Y translation error too large with missing stars: expected {}, got {}",
        dy,
        recovered_dy
    );
}

#[test]
fn test_outlier_rejection_combined() {
    // 10% spurious + 10% missing in target
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 55555);

    let dx = 40.0;
    let dy = -30.0;
    let target_stars = translate_stars(&ref_stars, dx, dy);

    // Remove 10% then add 10% spurious
    let target_modified = remove_random_stars(&target_stars, 0.1, 66666);
    let target_modified = add_spurious_stars(&target_modified, 10, 2000.0, 2000.0, 77777);

    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_modified)
        .expect("Registration should succeed despite combined outliers");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    assert!(
        (recovered_dx - dx).abs() < 1.5,
        "X translation error too large with combined outliers: expected {}, got {}",
        dx,
        recovered_dx
    );
    assert!(
        (recovered_dy - dy).abs() < 1.5,
        "Y translation error too large with combined outliers: expected {}, got {}",
        dy,
        recovered_dy
    );
}

#[test]
fn test_outlier_rejection_20_percent_spurious() {
    // 20% spurious stars - more aggressive test
    let ref_stars = generate_random_positions(80, 2000.0, 2000.0, 88888);

    let dx = 35.0;
    let dy = 25.0;
    let angle_rad = 0.5_f64.to_radians();
    let target_stars = transform_stars(&ref_stars, dx, dy, angle_rad, 1.0, 1000.0, 1000.0);

    // Add 16 spurious stars (20% of original)
    let target_with_spurious = add_spurious_stars(&target_stars, 16, 2000.0, 2000.0, 99999);

    let config = RegistrationConfig::builder()
        .with_rotation()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_with_spurious)
        .expect("Registration should succeed with 20% spurious stars");

    // Verify by applying transform
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let (tx, ty) = result.transform.apply(ref_star.0, ref_star.1);
        let error = ((tx - target_star.0).powi(2) + (ty - target_star.1).powi(2)).sqrt();
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 2.0,
        "Max transformation error too large with 20% spurious: {} pixels",
        max_error
    );
}

// ============================================================================
// Partial Overlap Tests
// ============================================================================

#[test]
fn test_partial_overlap_75_percent() {
    // 75% overlap - 25% of stars at edges won't match
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 11112);

    // Large translation causing 25% non-overlap
    let dx = 500.0; // 25% of 2000
    let dy = 0.0;

    let target_stars = translate_with_overlap(&ref_stars, dx, dy, 2000.0, 2000.0, 50.0);

    // Filter reference stars to only those that would be in target frame
    let ref_in_overlap: Vec<(f64, f64)> = ref_stars
        .iter()
        .filter(|(x, _)| *x + dx >= 50.0 && *x + dx <= 1950.0)
        .copied()
        .collect();

    assert!(
        target_stars.len() >= 70,
        "Expected at least 70 overlapping stars, got {}",
        target_stars.len()
    );

    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_in_overlap, &target_stars)
        .expect("Registration should succeed with 75% overlap");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    assert!(
        (recovered_dx - dx).abs() < 1.0,
        "X translation error with 75% overlap: expected {}, got {}",
        dx,
        recovered_dx
    );
    assert!(
        (recovered_dy - dy).abs() < 1.0,
        "Y translation error with 75% overlap: expected {}, got {}",
        dy,
        recovered_dy
    );
}

#[test]
fn test_partial_overlap_50_percent() {
    // 50% overlap - half the field doesn't match
    let ref_stars = generate_random_positions(120, 2000.0, 2000.0, 22223);

    // Large translation causing 50% non-overlap
    let dx = 1000.0; // 50% of 2000
    let dy = 0.0;

    let target_stars = translate_with_overlap(&ref_stars, dx, dy, 2000.0, 2000.0, 50.0);

    // Filter reference stars to only those that would be in target frame
    let ref_in_overlap: Vec<(f64, f64)> = ref_stars
        .iter()
        .filter(|(x, _)| *x + dx >= 50.0 && *x + dx <= 1950.0)
        .copied()
        .collect();

    assert!(
        target_stars.len() >= 50,
        "Expected at least 50 overlapping stars, got {}",
        target_stars.len()
    );

    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_in_overlap, &target_stars)
        .expect("Registration should succeed with 50% overlap");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    assert!(
        (recovered_dx - dx).abs() < 1.0,
        "X translation error with 50% overlap: expected {}, got {}",
        dx,
        recovered_dx
    );
    assert!(
        (recovered_dy - dy).abs() < 1.0,
        "Y translation error with 50% overlap: expected {}, got {}",
        dy,
        recovered_dy
    );
}

#[test]
fn test_partial_overlap_diagonal() {
    // Diagonal shift causing corner overlap
    let ref_stars = generate_random_positions(150, 2000.0, 2000.0, 33334);

    let dx = 400.0;
    let dy = 400.0;

    let target_stars = translate_with_overlap(&ref_stars, dx, dy, 2000.0, 2000.0, 50.0);

    let ref_in_overlap: Vec<(f64, f64)> = ref_stars
        .iter()
        .filter(|(x, y)| {
            *x + dx >= 50.0 && *x + dx <= 1950.0 && *y + dy >= 50.0 && *y + dy <= 1950.0
        })
        .copied()
        .collect();

    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_in_overlap, &target_stars)
        .expect("Registration should succeed with diagonal overlap");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    assert!(
        (recovered_dx - dx).abs() < 1.0,
        "X translation error with diagonal overlap: expected {}, got {}",
        dx,
        recovered_dx
    );
    assert!(
        (recovered_dy - dy).abs() < 1.0,
        "Y translation error with diagonal overlap: expected {}, got {}",
        dy,
        recovered_dy
    );
}

// ============================================================================
// Subpixel Accuracy Tests
// ============================================================================

#[test]
fn test_subpixel_translation_quarter_pixel() {
    // Test 0.25 pixel translation recovery
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 44445);

    let dx = 10.25;
    let dy = -5.75;
    let target_stars = translate_stars(&ref_stars, dx, dy);

    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(1.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    // Should recover subpixel translation within 0.1 pixel
    assert!(
        (recovered_dx - dx).abs() < 0.1,
        "Subpixel X error: expected {}, got {}, error {}",
        dx,
        recovered_dx,
        (recovered_dx - dx).abs()
    );
    assert!(
        (recovered_dy - dy).abs() < 0.1,
        "Subpixel Y error: expected {}, got {}, error {}",
        dy,
        recovered_dy,
        (recovered_dy - dy).abs()
    );
}

#[test]
fn test_subpixel_translation_half_pixel() {
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 55556);

    let dx = 20.5;
    let dy = -15.5;
    let target_stars = translate_stars(&ref_stars, dx, dy);

    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(1.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    assert!(
        (recovered_dx - dx).abs() < 0.1,
        "Half-pixel X error: expected {}, got {}",
        dx,
        recovered_dx
    );
    assert!(
        (recovered_dy - dy).abs() < 0.1,
        "Half-pixel Y error: expected {}, got {}",
        dy,
        recovered_dy
    );
}

#[test]
fn test_subpixel_rotation() {
    // Test 0.1 degree rotation recovery
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 66667);

    let angle_deg: f64 = 0.1;
    let angle_rad = angle_deg.to_radians();
    let target_stars = transform_stars(&ref_stars, 5.0, -3.0, angle_rad, 1.0, 1000.0, 1000.0);

    let config = RegistrationConfig::builder()
        .with_rotation()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(1.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    let recovered_angle = result.transform.rotation_angle();
    let angle_error_deg = (recovered_angle - angle_rad).abs().to_degrees();

    // Should recover 0.1 degree rotation within 0.01 degrees
    assert!(
        angle_error_deg < 0.01,
        "Subpixel rotation error: expected {} deg, got {} deg, error {} deg",
        angle_deg,
        recovered_angle.to_degrees(),
        angle_error_deg
    );
}

#[test]
fn test_subpixel_scale() {
    // Test 0.1% scale change recovery
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 77778);

    let scale = 1.001; // 0.1% scale change
    let target_stars = transform_stars(&ref_stars, 0.0, 0.0, 0.0, scale, 1000.0, 1000.0);

    let config = RegistrationConfig::builder()
        .with_scale()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(1.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    let recovered_scale = result.transform.scale_factor();
    let scale_error = (recovered_scale - scale).abs();

    // Should recover 0.1% scale within 0.05%
    assert!(
        scale_error < 0.0005,
        "Subpixel scale error: expected {}, got {}, error {}",
        scale,
        recovered_scale,
        scale_error
    );
}

// ============================================================================
// Minimum Star Count Tests
// ============================================================================

#[test]
fn test_minimum_stars_translation() {
    // Translation needs minimum 1 point, but for RANSAC we need more
    // Test with 6 stars (practical minimum)
    let ref_stars = generate_random_positions(6, 1000.0, 1000.0, 88889);

    let dx = 15.0;
    let dy = -10.0;
    let target_stars = translate_stars(&ref_stars, dx, dy);

    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(4)
        .min_matched_stars(3)
        .max_residual(2.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed with 6 stars");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    assert!(
        (recovered_dx - dx).abs() < 0.5,
        "Translation error with 6 stars: expected {}, got {}",
        dx,
        recovered_dx
    );
    assert!(
        (recovered_dy - dy).abs() < 0.5,
        "Translation error with 6 stars: expected {}, got {}",
        dy,
        recovered_dy
    );
}

#[test]
fn test_minimum_stars_similarity() {
    // Similarity needs minimum 2 points
    // Test with 8 stars
    let ref_stars = generate_random_positions(8, 1000.0, 1000.0, 99990);

    let dx = 10.0;
    let dy = -8.0;
    let angle_rad = 0.5_f64.to_radians();
    let scale = 1.01;
    let target_stars = transform_stars(&ref_stars, dx, dy, angle_rad, scale, 500.0, 500.0);

    let config = RegistrationConfig::builder()
        .with_scale()
        .min_stars(4)
        .min_matched_stars(3)
        .max_residual(2.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed with 8 stars");

    // Verify transform accuracy
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let (tx, ty) = result.transform.apply(ref_star.0, ref_star.1);
        let error = ((tx - target_star.0).powi(2) + (ty - target_star.1).powi(2)).sqrt();
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 1.0,
        "Max error with 8 stars: {} pixels",
        max_error
    );
}

#[test]
fn test_insufficient_stars_fails() {
    // Should fail with too few stars
    let ref_stars = generate_random_positions(3, 1000.0, 1000.0, 11110);
    let target_stars = translate_stars(&ref_stars, 10.0, 5.0);

    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(4)
        .min_matched_stars(3)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator.register_stars(&ref_stars, &target_stars);

    assert!(
        result.is_err(),
        "Registration should fail with only 3 stars"
    );
}

// ============================================================================
// Combined Disturbances (Stress Tests)
// ============================================================================

#[test]
fn test_stress_transform_noise_outliers() {
    // Transform + noise + 10% missing + 5% spurious
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 22221);

    let dx = 45.0;
    let dy = -30.0;
    let angle_rad = 1.0_f64.to_radians();
    let target_stars = transform_stars(&ref_stars, dx, dy, angle_rad, 1.0, 1000.0, 1000.0);

    // Add position noise
    let target_noisy = add_position_noise(&target_stars, 0.3, 33332);

    // Remove 10% and add 5% spurious
    let target_modified = remove_random_stars(&target_noisy, 0.1, 44443);
    let target_modified = add_spurious_stars(&target_modified, 5, 2000.0, 2000.0, 55554);

    let config = RegistrationConfig::builder()
        .with_rotation()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_modified)
        .expect("Registration should succeed under stress conditions");

    // Verify rotation
    let recovered_angle = result.transform.rotation_angle();
    let rotation_error = (recovered_angle - angle_rad).abs();

    assert!(
        rotation_error < 0.02,
        "Rotation error under stress: expected {} rad, got {} rad",
        angle_rad,
        recovered_angle
    );

    assert!(
        result.rms_error < 3.0,
        "RMS error under stress: {}",
        result.rms_error
    );
}

#[test]
fn test_stress_partial_overlap_with_noise() {
    // 60% overlap + noise + rotation
    let ref_stars = generate_random_positions(150, 2000.0, 2000.0, 66665);

    let dx = 800.0; // 40% shift
    let dy = 0.0;
    let angle_rad = 0.5_f64.to_radians();

    // Apply transform and filter to overlap region
    let target_stars: Vec<(f64, f64)> = ref_stars
        .iter()
        .map(|(x, y)| {
            // Apply rotation around center
            let cx = 1000.0;
            let cy = 1000.0;
            let rx = x - cx;
            let ry = y - cy;
            let cos_a = angle_rad.cos();
            let sin_a = angle_rad.sin();
            let new_x = cos_a * rx - sin_a * ry + cx + dx;
            let new_y = sin_a * rx + cos_a * ry + cy + dy;
            (new_x, new_y)
        })
        .filter(|(x, y)| *x >= 50.0 && *x <= 1950.0 && *y >= 50.0 && *y <= 1950.0)
        .collect();

    // Add noise
    let target_noisy = add_position_noise(&target_stars, 0.5, 77776);

    // Filter reference to overlapping region (accounting for transform)
    let ref_in_overlap: Vec<(f64, f64)> = ref_stars
        .iter()
        .filter(|(x, y)| {
            let cx = 1000.0;
            let cy = 1000.0;
            let rx = x - cx;
            let ry = y - cy;
            let cos_a = angle_rad.cos();
            let sin_a = angle_rad.sin();
            let new_x = cos_a * rx - sin_a * ry + cx + dx;
            let new_y = sin_a * rx + cos_a * ry + cy + dy;
            (50.0..=1950.0).contains(&new_x) && (50.0..=1950.0).contains(&new_y)
        })
        .copied()
        .collect();

    let config = RegistrationConfig::builder()
        .with_rotation()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_in_overlap, &target_noisy)
        .expect("Registration should succeed with partial overlap and noise");

    let recovered_angle = result.transform.rotation_angle();
    let rotation_error = (recovered_angle - angle_rad).abs();

    assert!(
        rotation_error < 0.03,
        "Rotation error with overlap+noise: {} vs {}",
        recovered_angle,
        angle_rad
    );
}

#[test]
fn test_stress_dense_field_large_transform() {
    // Dense field (200 stars) + large translation + scale change
    let ref_stars = generate_random_positions(200, 3000.0, 3000.0, 88887);

    let dx = 150.0;
    let dy = -100.0;
    let scale = 1.008;
    let target_stars = transform_stars(&ref_stars, dx, dy, 0.0, scale, 1500.0, 1500.0);

    let config = RegistrationConfig::builder()
        .with_scale()
        .min_stars(10)
        .min_matched_stars(8)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed with dense field");

    let recovered_scale = result.transform.scale_factor();
    let scale_error = (recovered_scale - scale).abs();

    assert!(
        scale_error < 0.002,
        "Scale error in dense field: expected {}, got {}",
        scale,
        recovered_scale
    );

    // Should match many stars in dense field
    assert!(
        result.num_inliers >= 100,
        "Expected many inliers in dense field, got {}",
        result.num_inliers
    );
}
