//! Robustness tests for registration.
//!
//! Tests for:
//! - Outlier rejection (spurious/missing stars)
//! - Partial overlap
//! - Subpixel accuracy
//! - Minimum star counts
//! - Combined disturbances (stress tests)

use crate::registration::{Config, TransformType, register};
use crate::star_detection::Star;
use crate::testing::synthetic::{
    add_spurious_star_list, add_star_noise, filter_stars_to_bounds, generate_random_stars,
    remove_random_star_list, transform_star_list, translate_star_list,
    translate_stars_with_overlap,
};
use glam::DVec2;

use super::helpers::{apply_affine, apply_homography};

// FWHM values that control max_sigma in registration:
// max_sigma = fwhm * 0.5, floor at 0.5
const FWHM_TIGHT: f32 = 1.34; // max_sigma ~0.67
const FWHM_NORMAL: f32 = 2.0; // max_sigma ~1.0
const FWHM_LOOSE: f32 = 3.34; // max_sigma ~1.67
const FWHM_SUBPIXEL: f32 = 0.66; // max_sigma ~0.33

// ============================================================================
// Outlier Rejection Tests
// ============================================================================

#[test]
fn test_outlier_rejection_spurious_stars() {
    // 10% spurious stars in target (false detections)
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 11111, FWHM_NORMAL);

    let dx = 30.0;
    let dy = -20.0;
    let target_stars = translate_star_list(&ref_stars, dx, dy);

    // Add 10 spurious stars (10% of original)
    let target_with_spurious =
        add_spurious_star_list(&target_stars, 10, 2000.0, 2000.0, 22222, FWHM_NORMAL);

    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_with_spurious, &config)
        .expect("Registration should succeed despite spurious stars");

    let recovered = result.transform.translation_components();

    assert!(
        (recovered.x - dx).abs() < 1.0,
        "X translation error too large with spurious stars: expected {}, got {}",
        dx,
        recovered.x
    );
    assert!(
        (recovered.y - dy).abs() < 1.0,
        "Y translation error too large with spurious stars: expected {}, got {}",
        dy,
        recovered.y
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
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 33333, FWHM_NORMAL);

    let dx = 25.0;
    let dy = 15.0;
    let target_stars = translate_star_list(&ref_stars, dx, dy);

    // Remove ~10% of stars
    let target_with_missing = remove_random_star_list(&target_stars, 0.1, 44444);

    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_with_missing, &config)
        .expect("Registration should succeed despite missing stars");

    let recovered = result.transform.translation_components();

    assert!(
        (recovered.x - dx).abs() < 1.0,
        "X translation error too large with missing stars: expected {}, got {}",
        dx,
        recovered.x
    );
    assert!(
        (recovered.y - dy).abs() < 1.0,
        "Y translation error too large with missing stars: expected {}, got {}",
        dy,
        recovered.y
    );
}

#[test]
fn test_outlier_rejection_combined() {
    // 10% spurious + 10% missing in target
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 55555, FWHM_NORMAL);

    let dx = 40.0;
    let dy = -30.0;
    let target_stars = translate_star_list(&ref_stars, dx, dy);

    // Remove 10% then add 10% spurious
    let target_modified = remove_random_star_list(&target_stars, 0.1, 66666);
    let target_modified =
        add_spurious_star_list(&target_modified, 10, 2000.0, 2000.0, 77777, FWHM_NORMAL);

    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_modified, &config)
        .expect("Registration should succeed despite combined outliers");

    let recovered = result.transform.translation_components();

    assert!(
        (recovered.x - dx).abs() < 1.5,
        "X translation error too large with combined outliers: expected {}, got {}",
        dx,
        recovered.x
    );
    assert!(
        (recovered.y - dy).abs() < 1.5,
        "Y translation error too large with combined outliers: expected {}, got {}",
        dy,
        recovered.y
    );
}

#[test]
fn test_outlier_rejection_20_percent_spurious() {
    // 20% spurious stars - more aggressive test
    let ref_stars = generate_random_stars(80, 2000.0, 2000.0, 88888, FWHM_NORMAL);

    let dx = 35.0;
    let dy = 25.0;
    let angle_rad = 0.5_f64.to_radians();
    let target_stars = transform_star_list(&ref_stars, dx, dy, angle_rad, 1.0, 1000.0, 1000.0);

    // Add 16 spurious stars (20% of original)
    let target_with_spurious =
        add_spurious_star_list(&target_stars, 16, 2000.0, 2000.0, 99999, FWHM_NORMAL);

    let config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_with_spurious, &config)
        .expect("Registration should succeed with 20% spurious stars");

    // Verify by applying transform
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(ref_star.pos);
        let error = t.distance(target_star.pos);
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
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 11112, FWHM_NORMAL);

    // Large translation causing 25% non-overlap
    let dx = 500.0; // 25% of 2000
    let dy = 0.0;

    let target_stars = translate_stars_with_overlap(&ref_stars, dx, dy, 2000.0, 2000.0, 50.0);

    // Filter reference stars to only those that would be in target frame
    let ref_in_overlap =
        filter_stars_to_bounds(&ref_stars, -dx + 50.0, 2000.0 - dx - 50.0, 50.0, 1950.0);

    assert!(
        target_stars.len() >= 70,
        "Expected at least 70 overlapping stars, got {}",
        target_stars.len()
    );

    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_in_overlap, &target_stars, &config)
        .expect("Registration should succeed with 75% overlap");

    let recovered = result.transform.translation_components();

    assert!(
        (recovered.x - dx).abs() < 1.0,
        "X translation error with 75% overlap: expected {}, got {}",
        dx,
        recovered.x
    );
    assert!(
        (recovered.y - dy).abs() < 1.0,
        "Y translation error with 75% overlap: expected {}, got {}",
        dy,
        recovered.y
    );
}

#[test]
fn test_partial_overlap_50_percent() {
    // 50% overlap - half the field doesn't match
    let ref_stars = generate_random_stars(120, 2000.0, 2000.0, 22223, FWHM_NORMAL);

    // Large translation causing 50% non-overlap
    let dx = 1000.0; // 50% of 2000
    let dy = 0.0;

    let target_stars = translate_stars_with_overlap(&ref_stars, dx, dy, 2000.0, 2000.0, 50.0);

    // Filter reference stars to only those that would be in target frame
    let ref_in_overlap =
        filter_stars_to_bounds(&ref_stars, -dx + 50.0, 2000.0 - dx - 50.0, 50.0, 1950.0);

    assert!(
        target_stars.len() >= 50,
        "Expected at least 50 overlapping stars, got {}",
        target_stars.len()
    );

    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_in_overlap, &target_stars, &config)
        .expect("Registration should succeed with 50% overlap");

    let recovered = result.transform.translation_components();

    assert!(
        (recovered.x - dx).abs() < 1.0,
        "X translation error with 50% overlap: expected {}, got {}",
        dx,
        recovered.x
    );
    assert!(
        (recovered.y - dy).abs() < 1.0,
        "Y translation error with 50% overlap: expected {}, got {}",
        dy,
        recovered.y
    );
}

#[test]
fn test_partial_overlap_diagonal() {
    // Diagonal shift causing corner overlap
    let ref_stars = generate_random_stars(150, 2000.0, 2000.0, 33334, FWHM_NORMAL);

    let dx = 400.0;
    let dy = 400.0;

    let target_stars = translate_stars_with_overlap(&ref_stars, dx, dy, 2000.0, 2000.0, 50.0);

    let ref_in_overlap = filter_stars_to_bounds(
        &ref_stars,
        -dx + 50.0,
        2000.0 - dx - 50.0,
        -dy + 50.0,
        2000.0 - dy - 50.0,
    );

    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_in_overlap, &target_stars, &config)
        .expect("Registration should succeed with diagonal overlap");

    let recovered = result.transform.translation_components();
    let recovered_dx = recovered.x;
    let recovered_dy = recovered.y;

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
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 44445, FWHM_SUBPIXEL);

    let dx = 10.25;
    let dy = -5.75;
    let target_stars = translate_star_list(&ref_stars, dx, dy);

    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config).expect("Registration should succeed");

    let recovered = result.transform.translation_components();
    let recovered_dx = recovered.x;
    let recovered_dy = recovered.y;

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
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 55556, FWHM_SUBPIXEL);

    let dx = 20.5;
    let dy = -15.5;
    let target_stars = translate_star_list(&ref_stars, dx, dy);

    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config).expect("Registration should succeed");

    let recovered = result.transform.translation_components();
    let recovered_dx = recovered.x;
    let recovered_dy = recovered.y;

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
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 66667, FWHM_SUBPIXEL);

    let angle_deg: f64 = 0.1;
    let angle_rad = angle_deg.to_radians();
    let target_stars = transform_star_list(&ref_stars, 5.0, -3.0, angle_rad, 1.0, 1000.0, 1000.0);

    let config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config).expect("Registration should succeed");

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
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 77778, FWHM_SUBPIXEL);

    let scale = 1.001; // 0.1% scale change
    let target_stars = transform_star_list(&ref_stars, 0.0, 0.0, 0.0, scale, 1000.0, 1000.0);

    let config = Config {
        transform_type: TransformType::Similarity,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config).expect("Registration should succeed");

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
    let ref_stars = generate_random_stars(6, 1000.0, 1000.0, 88889, FWHM_TIGHT);

    let dx = 15.0;
    let dy = -10.0;
    let target_stars = translate_star_list(&ref_stars, dx, dy);

    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 4,
        min_matches: 3,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed with 6 stars");

    let recovered = result.transform.translation_components();
    let recovered_dx = recovered.x;
    let recovered_dy = recovered.y;

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
    let ref_stars = generate_random_stars(8, 1000.0, 1000.0, 99990, FWHM_TIGHT);

    let dx = 10.0;
    let dy = -8.0;
    let angle_rad = 0.5_f64.to_radians();
    let scale = 1.01;
    let target_stars = transform_star_list(&ref_stars, dx, dy, angle_rad, scale, 500.0, 500.0);

    let config = Config {
        transform_type: TransformType::Similarity,
        min_stars: 4,
        min_matches: 3,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed with 8 stars");

    // Verify transform accuracy
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(ref_star.pos);
        let error = t.distance(target_star.pos);
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
    let ref_stars = generate_random_stars(3, 1000.0, 1000.0, 11110, FWHM_NORMAL);
    let target_stars = translate_star_list(&ref_stars, 10.0, 5.0);

    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 4,
        min_matches: 3,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config);

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
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 22221, FWHM_NORMAL);

    let dx = 45.0;
    let dy = -30.0;
    let angle_rad = 1.0_f64.to_radians();
    let target_stars = transform_star_list(&ref_stars, dx, dy, angle_rad, 1.0, 1000.0, 1000.0);

    // Add position noise
    let target_noisy = add_star_noise(&target_stars, 0.3, 33332);

    // Remove 10% and add 5% spurious
    let target_modified = remove_random_star_list(&target_noisy, 0.1, 44443);
    let target_modified =
        add_spurious_star_list(&target_modified, 5, 2000.0, 2000.0, 55554, FWHM_NORMAL);

    let config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_modified, &config)
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
    let ref_stars = generate_random_stars(150, 2000.0, 2000.0, 66665, FWHM_NORMAL);

    let dx = 800.0; // 40% shift
    let dy = 0.0;
    let angle_rad = 0.5_f64.to_radians();

    // Apply transform and filter to overlap region
    let center = DVec2::new(1000.0, 1000.0);
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();
    let offset = DVec2::new(dx, dy);

    let target_stars: Vec<Star> = ref_stars
        .iter()
        .map(|s| {
            // Apply rotation around center
            let r = s.pos - center;
            let new_x = cos_a * r.x - sin_a * r.y + center.x + offset.x;
            let new_y = sin_a * r.x + cos_a * r.y + center.y + offset.y;
            Star {
                pos: DVec2::new(new_x, new_y),
                ..*s
            }
        })
        .filter(|s| s.pos.x >= 50.0 && s.pos.x <= 1950.0 && s.pos.y >= 50.0 && s.pos.y <= 1950.0)
        .collect();

    // Add noise
    let target_noisy = add_star_noise(&target_stars, 0.5, 77776);

    // Filter reference to overlapping region (accounting for transform)
    let ref_in_overlap: Vec<Star> = ref_stars
        .iter()
        .filter(|s| {
            let r = s.pos - center;
            let new_x = cos_a * r.x - sin_a * r.y + center.x + offset.x;
            let new_y = sin_a * r.x + cos_a * r.y + center.y + offset.y;
            (50.0..=1950.0).contains(&new_x) && (50.0..=1950.0).contains(&new_y)
        })
        .cloned()
        .collect();

    let config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_in_overlap, &target_noisy, &config)
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
    let ref_stars = generate_random_stars(200, 3000.0, 3000.0, 88887, FWHM_NORMAL);

    let dx = 150.0;
    let dy = -100.0;
    let scale = 1.008;
    let target_stars = transform_star_list(&ref_stars, dx, dy, 0.0, scale, 1500.0, 1500.0);

    let config = Config {
        transform_type: TransformType::Similarity,
        min_stars: 10,
        min_matches: 8,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config)
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

// ============================================================================
// Large Rotation Tests
// ============================================================================

#[test]
fn test_large_rotation_45_degrees() {
    // 45 degree rotation - tests trig at non-trivial angles
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 10001, FWHM_NORMAL);

    let angle_deg: f64 = 45.0;
    let angle_rad = angle_deg.to_radians();
    let dx = 20.0;
    let dy = -15.0;
    let center_x = 1000.0;
    let center_y = 1000.0;

    let target_stars = transform_star_list(&ref_stars, dx, dy, angle_rad, 1.0, center_x, center_y);

    let config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 6,
        min_matches: 4,
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed with 45° rotation");

    let recovered_angle = result.transform.rotation_angle();
    let rotation_error_deg = (recovered_angle - angle_rad).abs().to_degrees();

    assert!(
        rotation_error_deg < 0.1,
        "45° rotation error too large: expected {}°, got {}°, error {}°",
        angle_deg,
        recovered_angle.to_degrees(),
        rotation_error_deg
    );

    // Validate transform accuracy
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(ref_star.pos);
        let error = t.distance(target_star.pos);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 1.0,
        "Max transformation error with 45° rotation: {} pixels",
        max_error
    );
}

#[test]
fn test_large_rotation_90_degrees() {
    // 90 degree rotation - edge case for atan2
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 10002, FWHM_NORMAL);

    let angle_deg: f64 = 90.0;
    let angle_rad = angle_deg.to_radians();
    let dx = 10.0;
    let dy = 10.0;
    let center_x = 1000.0;
    let center_y = 1000.0;

    let target_stars = transform_star_list(&ref_stars, dx, dy, angle_rad, 1.0, center_x, center_y);

    let config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 6,
        min_matches: 4,
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed with 90° rotation");

    let recovered_angle = result.transform.rotation_angle();
    let rotation_error_deg = (recovered_angle - angle_rad).abs().to_degrees();

    assert!(
        rotation_error_deg < 0.1,
        "90° rotation error too large: expected {}°, got {}°, error {}°",
        angle_deg,
        recovered_angle.to_degrees(),
        rotation_error_deg
    );

    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(ref_star.pos);
        let error = t.distance(target_star.pos);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 1.0,
        "Max transformation error with 90° rotation: {} pixels",
        max_error
    );
}

#[test]
fn test_large_rotation_negative_45_degrees() {
    // Negative rotation to test both directions
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 10003, FWHM_NORMAL);

    let angle_deg: f64 = -45.0;
    let angle_rad = angle_deg.to_radians();
    let center_x = 1000.0;
    let center_y = 1000.0;

    let target_stars =
        transform_star_list(&ref_stars, 0.0, 0.0, angle_rad, 1.0, center_x, center_y);

    let config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 6,
        min_matches: 4,
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed with -45° rotation");

    let recovered_angle = result.transform.rotation_angle();
    let rotation_error_deg = (recovered_angle - angle_rad).abs().to_degrees();

    assert!(
        rotation_error_deg < 0.1,
        "-45° rotation error too large: expected {}°, got {}°, error {}°",
        angle_deg,
        recovered_angle.to_degrees(),
        rotation_error_deg
    );
}

// ============================================================================
// Extreme Scale Tests
// ============================================================================

#[test]
fn test_extreme_scale_2x() {
    // 2x scale factor (zoom in)
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 20001, FWHM_LOOSE);

    let scale = 2.0;
    let center_x = 1000.0;
    let center_y = 1000.0;

    let target_stars = transform_star_list(&ref_stars, 0.0, 0.0, 0.0, scale, center_x, center_y);

    let config = Config {
        transform_type: TransformType::Similarity,
        min_stars: 6,
        min_matches: 4,
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };

    // Allow more residual for large scale (FWHM_LOOSE -> max_sigma ~1.67)
    let result = register(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed with 2x scale");

    let recovered_scale = result.transform.scale_factor();
    let scale_error = (recovered_scale - scale).abs();

    assert!(
        scale_error < 0.01,
        "2x scale error too large: expected {}, got {}, error {}",
        scale,
        recovered_scale,
        scale_error
    );

    // Validate transform accuracy
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(ref_star.pos);
        let error = t.distance(target_star.pos);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 2.0,
        "Max transformation error with 2x scale: {} pixels",
        max_error
    );
}

#[test]
fn test_extreme_scale_half() {
    // 0.5x scale factor (zoom out)
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 20002, FWHM_LOOSE);

    let scale = 0.5;
    let center_x = 1000.0;
    let center_y = 1000.0;

    let target_stars = transform_star_list(&ref_stars, 0.0, 0.0, 0.0, scale, center_x, center_y);

    let config = Config {
        transform_type: TransformType::Similarity,
        min_stars: 6,
        min_matches: 4,
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed with 0.5x scale");

    let recovered_scale = result.transform.scale_factor();
    let scale_error = (recovered_scale - scale).abs();

    assert!(
        scale_error < 0.01,
        "0.5x scale error too large: expected {}, got {}, error {}",
        scale,
        recovered_scale,
        scale_error
    );

    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(ref_star.pos);
        let error = t.distance(target_star.pos);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 2.0,
        "Max transformation error with 0.5x scale: {} pixels",
        max_error
    );
}

#[test]
fn test_extreme_scale_with_rotation() {
    // Combined extreme scale (1.5x) with rotation (30°)
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 20003, FWHM_LOOSE);

    let scale = 1.5;
    let angle_deg: f64 = 30.0;
    let angle_rad = angle_deg.to_radians();
    let dx = 50.0;
    let dy = -30.0;
    let center_x = 1000.0;
    let center_y = 1000.0;

    let target_stars =
        transform_star_list(&ref_stars, dx, dy, angle_rad, scale, center_x, center_y);

    let config = Config {
        transform_type: TransformType::Similarity,
        min_stars: 6,
        min_matches: 4,
        max_rotation: None,
        scale_range: None,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed with 1.5x scale + 30° rotation");

    let recovered_scale = result.transform.scale_factor();
    let scale_error = (recovered_scale - scale).abs();

    assert!(
        scale_error < 0.01,
        "Combined scale error: expected {}, got {}",
        scale,
        recovered_scale
    );

    let recovered_angle = result.transform.rotation_angle();
    let rotation_error_deg = (recovered_angle - angle_rad).abs().to_degrees();

    assert!(
        rotation_error_deg < 0.1,
        "Combined rotation error: expected {}°, got {}°",
        angle_deg,
        recovered_angle.to_degrees()
    );
}

// ============================================================================
// Affine Robustness Tests
// ============================================================================

#[test]
fn test_affine_with_outliers() {
    // Affine transform with differential scale + shear, plus 15% spurious stars
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 30001, FWHM_LOOSE);

    let scale_x = 1.01;
    let scale_y = 0.99;
    let shear = 0.005;
    let dx = 40.0;
    let dy = -25.0;

    let affine_params = [scale_x, shear, dx, 0.0, scale_y, dy];
    let target_stars = apply_affine(&ref_stars, affine_params);

    // Add 15% spurious stars
    let target_with_spurious =
        add_spurious_star_list(&target_stars, 15, 2000.0, 2000.0, 30002, FWHM_LOOSE);

    let config = Config {
        transform_type: TransformType::Affine,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_with_spurious, &config)
        .expect("Affine registration should succeed with 15% spurious stars");

    assert_eq!(result.transform.transform_type, TransformType::Affine);

    // Validate transform accuracy on original (non-spurious) stars
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(ref_star.pos);
        let error = t.distance(target_star.pos);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 2.0,
        "Max affine transformation error with outliers: {} pixels",
        max_error
    );
}

#[test]
fn test_affine_with_noise_and_missing() {
    // Affine with noise + 10% missing stars
    let ref_stars = generate_random_stars(100, 2000.0, 2000.0, 30003, FWHM_LOOSE);

    let scale_x = 1.005;
    let scale_y = 0.995;
    let shear = 0.003;
    let dx = 30.0;
    let dy = 20.0;

    let affine_params = [scale_x, shear, dx, 0.0, scale_y, dy];
    let target_stars = apply_affine(&ref_stars, affine_params);

    // Add noise
    let target_noisy = add_star_noise(&target_stars, 0.4, 30004);
    // Remove 10%
    let target_modified = remove_random_star_list(&target_noisy, 0.1, 30005);

    let config = Config {
        transform_type: TransformType::Affine,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_modified, &config)
        .expect("Affine registration should succeed with noise and missing stars");

    assert_eq!(result.transform.transform_type, TransformType::Affine);
    assert!(
        result.rms_error < 3.0,
        "Affine RMS error with noise: {}",
        result.rms_error
    );
}

// ============================================================================
// Homography Robustness Tests
// ============================================================================

#[test]
fn test_homography_with_outliers() {
    // Homography with perspective + 10% spurious stars
    let ref_stars = generate_random_stars(120, 2000.0, 2000.0, 40001, FWHM_LOOSE);

    let angle_rad = 0.3_f64.to_radians();
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();
    let dx = 30.0;
    let dy = -20.0;
    let h6 = 0.00002;
    let h7 = 0.00001;

    let homography_params = [cos_a, -sin_a, dx, sin_a, cos_a, dy, h6, h7];
    let target_stars = apply_homography(&ref_stars, homography_params);

    // Add 10% spurious
    let target_with_spurious =
        add_spurious_star_list(&target_stars, 12, 2000.0, 2000.0, 40002, FWHM_LOOSE);

    let config = Config {
        transform_type: TransformType::Homography,
        min_stars: 8,
        min_matches: 6,
        ..Default::default()
    };

    let result = register(&ref_stars, &target_with_spurious, &config)
        .expect("Homography registration should succeed with outliers");

    assert_eq!(result.transform.transform_type, TransformType::Homography);

    // Validate transform accuracy
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(ref_star.pos);
        let error = t.distance(target_star.pos);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 3.0,
        "Max homography transformation error with outliers: {} pixels",
        max_error
    );
}

#[test]
fn test_homography_with_noise_and_partial_overlap() {
    // Homography with noise + 70% overlap
    let ref_stars = generate_random_stars(150, 2000.0, 2000.0, 40003, FWHM_LOOSE);

    let dx = 600.0; // 30% shift = 70% overlap
    let h6 = 0.000015;
    let h7 = 0.000008;

    let homography_params = [1.0, 0.0, dx, 0.0, 1.0, 0.0, h6, h7];
    let target_stars = apply_homography(&ref_stars, homography_params);

    // Filter to overlap region
    let target_in_bounds = filter_stars_to_bounds(&target_stars, 50.0, 1950.0, 50.0, 1950.0);

    // Add noise
    let target_noisy = add_star_noise(&target_in_bounds, 0.3, 40004);

    // Filter reference to overlapping region
    let ref_in_overlap: Vec<Star> = ref_stars
        .iter()
        .filter(|s| {
            let w = h6 * s.pos.x + h7 * s.pos.y + 1.0;
            let new_x = (s.pos.x + dx) / w;
            let new_y = s.pos.y / w;
            (50.0..=1950.0).contains(&new_x) && (50.0..=1950.0).contains(&new_y)
        })
        .cloned()
        .collect();

    let config = Config {
        transform_type: TransformType::Homography,
        min_stars: 8,
        min_matches: 6,
        ..Default::default()
    };

    let result = register(&ref_in_overlap, &target_noisy, &config)
        .expect("Homography should succeed with noise and partial overlap");

    assert_eq!(result.transform.transform_type, TransformType::Homography);
    assert!(
        result.rms_error < 3.0,
        "Homography RMS error with noise and overlap: {}",
        result.rms_error
    );
}
