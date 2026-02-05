//! Tests for transform type estimation using synthetic star positions.
//!
//! These tests verify that the registrator correctly estimates transforms
//! from known star position correspondences (without actual image data).
//!
//! Tests for all TransformType variants:
//! - Translation (2 DOF)
//! - Euclidean (3 DOF: translation + rotation)
//! - Similarity (4 DOF: translation + rotation + uniform scale)
//! - Affine (6 DOF: handles differential scaling and shear)
//! - Homography (8 DOF: handles perspective)

use crate::registration::{Config, TransformType, register_positions};
use crate::testing::synthetic::{generate_random_positions, transform_stars, translate_stars};
use glam::DVec2;

#[test]
fn test_registration_translation_only() {
    // Generate reference star field
    let ref_stars = generate_random_positions(50, 1000.0, 1000.0, 12345);

    // Apply known translation
    let dx = 25.5;
    let dy = -15.3;
    let target_stars = translate_stars(&ref_stars, dx, dy);

    // Configure registration for translation only
    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 0.67,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    // Extract recovered translation
    let recovered = result.transform.translation_components();

    // Validate translation (should be very close to original)
    let dx_error = (recovered.x - dx).abs();
    let dy_error = (recovered.y - dy).abs();

    assert!(
        dx_error < 0.1,
        "X translation error too large: expected {}, got {}, error {}",
        dx,
        recovered.x,
        dx_error
    );
    assert!(
        dy_error < 0.1,
        "Y translation error too large: expected {}, got {}, error {}",
        dy,
        recovered.y,
        dy_error
    );

    // Check RMS error is small
    assert!(
        result.rms_error < 0.5,
        "RMS error too large: {}",
        result.rms_error
    );

    // Check we matched most stars
    assert!(
        result.num_inliers >= 40,
        "Too few inliers: {}",
        result.num_inliers
    );
}

#[test]
fn test_registration_similarity_transform() {
    // Generate reference star field
    let ref_stars = generate_random_positions(80, 2000.0, 2000.0, 54321);

    // Apply known similarity transform (translation + rotation + scale)
    let dx = 30.0;
    let dy = -20.0;
    let angle_deg: f64 = 0.5; // Small rotation
    let angle_rad = angle_deg.to_radians();
    let scale = 1.002; // Small scale change
    let center_x = 1000.0;
    let center_y = 1000.0;

    let target_stars = transform_stars(&ref_stars, dx, dy, angle_rad, scale, center_x, center_y);

    // Configure registration with scale
    let config = Config {
        transform_type: TransformType::Similarity,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 1.0,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    // Validate by applying the recovered transform to reference stars
    // and checking they match target stars
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let transformed = result.transform.apply(*ref_star);
        let error = transformed.distance(*target_star);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 1.0,
        "Max transformation error too large: {} pixels",
        max_error
    );

    // Validate rotation is approximately correct
    let recovered_angle = result.transform.rotation_angle();
    let rotation_error = (recovered_angle - angle_rad).abs();
    assert!(
        rotation_error < 0.01,
        "Rotation error too large: expected {} rad, got {} rad, error {}",
        angle_rad,
        recovered_angle,
        rotation_error
    );

    // Validate scale is approximately correct
    let recovered_scale = result.transform.scale_factor();
    let scale_error = (recovered_scale - scale).abs();
    assert!(
        scale_error < 0.001,
        "Scale error too large: expected {}, got {}, error {}",
        scale,
        recovered_scale,
        scale_error
    );

    // Check RMS error is small
    assert!(
        result.rms_error < 1.0,
        "RMS error too large: {}",
        result.rms_error
    );
}

#[test]
fn test_registration_with_noise() {
    // Generate reference star field
    let ref_stars = generate_random_positions(100, 1500.0, 1500.0, 99999);

    // Apply translation with some noise in star positions
    let dx = 50.0;
    let dy = 35.0;

    let mut state = 11111u64;
    let mut next_random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((state >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0 // -1 to 1
    };

    let target_stars: Vec<DVec2> = ref_stars
        .iter()
        .map(|p| {
            let noise_x = next_random() * 0.5; // +/- 0.5 pixel noise
            let noise_y = next_random() * 0.5;
            DVec2::new(p.x + dx + noise_x, p.y + dy + noise_y)
        })
        .collect();

    // Configure registration
    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 1.0, // Allow more residual due to noise
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    // Extract recovered translation
    let recovered = result.transform.translation_components();

    // Validate translation (allow more error due to noise)
    let dx_error = (recovered.x - dx).abs();
    let dy_error = (recovered.y - dy).abs();

    assert!(
        dx_error < 1.0,
        "X translation error too large: expected {}, got {}, error {}",
        dx,
        recovered.x,
        dx_error
    );
    assert!(
        dy_error < 1.0,
        "Y translation error too large: expected {}, got {}, error {}",
        dy,
        recovered.y,
        dy_error
    );

    // RMS error will be higher due to noise, but should still be reasonable
    assert!(
        result.rms_error < 2.0,
        "RMS error too large: {}",
        result.rms_error
    );
}

#[test]
fn test_registration_large_translation() {
    // Generate reference star field
    let ref_stars = generate_random_positions(60, 2000.0, 2000.0, 77777);

    // Apply large translation (simulating significant pointing offset)
    let dx = 200.0;
    let dy = -150.0;
    let target_stars = translate_stars(&ref_stars, dx, dy);

    // Configure registration
    let config = Config {
        transform_type: TransformType::Translation,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 0.67,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    let recovered = result.transform.translation_components();

    let dx_error = (recovered.x - dx).abs();
    let dy_error = (recovered.y - dy).abs();

    assert!(
        dx_error < 0.1,
        "X translation error too large: {}",
        dx_error
    );
    assert!(
        dy_error < 0.1,
        "Y translation error too large: {}",
        dy_error
    );
}

#[test]
fn test_registration_transform_display() {
    let ref_stars = generate_random_positions(50, 1000.0, 1000.0, 12345);
    let target_stars = translate_stars(&ref_stars, 10.0, -5.0);

    let config = Config {
        transform_type: TransformType::Similarity,
        min_stars: 6,
        min_matches: 4,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    // Test that Display works
    let display_str = format!("{}", result.transform);
    assert!(
        display_str.contains("dx="),
        "Display should contain dx: {}",
        display_str
    );
    assert!(
        display_str.contains("dy="),
        "Display should contain dy: {}",
        display_str
    );
}

// ============================================================================
// TransformType::Euclidean tests (translation + rotation, no scale)
// ============================================================================

#[test]
fn test_registration_euclidean_rotation_only() {
    // Generate reference star field
    let ref_stars = generate_random_positions(80, 2000.0, 2000.0, 11111);

    // Apply pure rotation around image center (no translation, no scale)
    let angle_deg: f64 = 1.5;
    let angle_rad = angle_deg.to_radians();
    let center_x = 1000.0;
    let center_y = 1000.0;

    // transform_stars with scale=1.0 and dx=dy=0 gives pure rotation
    let target_stars = transform_stars(&ref_stars, 0.0, 0.0, angle_rad, 1.0, center_x, center_y);

    let config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 0.67,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    assert_eq!(result.transform.transform_type, TransformType::Euclidean);

    // Validate rotation
    let recovered_angle = result.transform.rotation_angle();
    let rotation_error = (recovered_angle - angle_rad).abs();
    assert!(
        rotation_error < 0.001,
        "Rotation error too large: expected {} rad, got {} rad, error {}",
        angle_rad,
        recovered_angle,
        rotation_error
    );

    // Scale should be ~1.0 for Euclidean
    let recovered_scale = result.transform.scale_factor();
    assert!(
        (recovered_scale - 1.0).abs() < 0.001,
        "Scale should be 1.0 for Euclidean, got {}",
        recovered_scale
    );

    assert!(
        result.rms_error < 0.5,
        "RMS error too large: {}",
        result.rms_error
    );
}

#[test]
fn test_registration_euclidean_translation_and_rotation() {
    let ref_stars = generate_random_positions(70, 1500.0, 1500.0, 22222);

    let dx = 45.0;
    let dy = -30.0;
    let angle_deg: f64 = 0.8;
    let angle_rad = angle_deg.to_radians();
    let center_x = 750.0;
    let center_y = 750.0;

    let target_stars = transform_stars(&ref_stars, dx, dy, angle_rad, 1.0, center_x, center_y);

    let config = Config {
        transform_type: TransformType::Euclidean,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 0.67,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    assert_eq!(result.transform.transform_type, TransformType::Euclidean);

    // Validate by applying transform to all reference stars
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(*ref_star);
        let error = t.distance(*target_star);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 0.5,
        "Max transformation error too large: {} pixels",
        max_error
    );

    let recovered_angle = result.transform.rotation_angle();
    let rotation_error = (recovered_angle - angle_rad).abs();
    assert!(
        rotation_error < 0.01,
        "Rotation error too large: {} vs {}",
        recovered_angle,
        angle_rad
    );
}

// ============================================================================
// TransformType::Affine tests (6 DOF: differential scaling, shear)
// ============================================================================

/// Apply an affine transform to star positions.
/// Affine: [a, b, tx, c, d, ty] where the transform is:
/// x' = a*x + b*y + tx
/// y' = c*x + d*y + ty
fn apply_affine(stars: &[DVec2], params: [f64; 6]) -> Vec<DVec2> {
    let [a, b, tx, c, d, ty] = params;
    stars
        .iter()
        .map(|p| DVec2::new(a * p.x + b * p.y + tx, c * p.x + d * p.y + ty))
        .collect()
}

#[test]
fn test_registration_affine_differential_scale() {
    // Test affine with different X and Y scales (anisotropic scaling)
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 33333);

    // Affine with different X/Y scales: scale_x=1.003, scale_y=0.998
    let scale_x = 1.003;
    let scale_y = 0.998;
    let dx = 20.0;
    let dy = -15.0;

    let affine_params = [scale_x, 0.0, dx, 0.0, scale_y, dy];
    let target_stars = apply_affine(&ref_stars, affine_params);

    let config = Config {
        transform_type: TransformType::Affine,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 1.0,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    assert_eq!(result.transform.transform_type, TransformType::Affine);

    // Validate by applying transform
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(*ref_star);
        let error = t.distance(*target_star);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 1.0,
        "Max transformation error too large: {} pixels",
        max_error
    );

    assert!(
        result.rms_error < 1.0,
        "RMS error too large: {}",
        result.rms_error
    );
}

#[test]
fn test_registration_affine_with_shear() {
    // Test affine with shear component
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 44444);

    // Affine with small shear: shear_x = 0.002
    let shear = 0.002;
    let dx = 30.0;
    let dy = 25.0;

    // [a, b, tx, c, d, ty] where b is shear in x direction
    let affine_params = [1.0, shear, dx, 0.0, 1.0, dy];
    let target_stars = apply_affine(&ref_stars, affine_params);

    let config = Config {
        transform_type: TransformType::Affine,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 1.0,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    assert_eq!(result.transform.transform_type, TransformType::Affine);

    // Validate transformation accuracy
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(*ref_star);
        let error = t.distance(*target_star);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 1.0,
        "Max transformation error too large: {} pixels",
        max_error
    );
}

#[test]
fn test_registration_affine_rotation_and_differential_scale() {
    // Combined rotation with differential scaling
    let ref_stars = generate_random_positions(100, 2000.0, 2000.0, 55555);

    let angle_rad = 0.3_f64.to_radians();
    let scale_x = 1.002;
    let scale_y = 0.999;
    let dx = 40.0;
    let dy = -20.0;

    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();

    // Rotation matrix scaled differently in X and Y
    let affine_params = [
        scale_x * cos_a,
        -scale_y * sin_a,
        dx,
        scale_x * sin_a,
        scale_y * cos_a,
        dy,
    ];
    let target_stars = apply_affine(&ref_stars, affine_params);

    let config = Config {
        transform_type: TransformType::Affine,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 1.0,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    assert_eq!(result.transform.transform_type, TransformType::Affine);

    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(*ref_star);
        let error = t.distance(*target_star);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 1.0,
        "Max transformation error too large: {} pixels",
        max_error
    );

    assert!(
        result.rms_error < 1.0,
        "RMS error too large: {}",
        result.rms_error
    );
}

// ============================================================================
// TransformType::Homography tests (8 DOF: perspective)
// ============================================================================

/// Apply a homography (projective transform) to star positions.
/// H = [h0, h1, h2, h3, h4, h5, h6, h7, 1.0]
/// x' = (h0*x + h1*y + h2) / (h6*x + h7*y + 1)
/// y' = (h3*x + h4*y + h5) / (h6*x + h7*y + 1)
fn apply_homography(stars: &[DVec2], params: [f64; 8]) -> Vec<DVec2> {
    stars
        .iter()
        .map(|p| {
            let w = params[6] * p.x + params[7] * p.y + 1.0;
            let x_prime = (params[0] * p.x + params[1] * p.y + params[2]) / w;
            let y_prime = (params[3] * p.x + params[4] * p.y + params[5]) / w;
            DVec2::new(x_prime, y_prime)
        })
        .collect()
}

#[test]
fn test_registration_homography_mild_perspective() {
    // Test homography with mild perspective distortion
    let ref_stars = generate_random_positions(120, 2000.0, 2000.0, 66666);

    // Mild perspective: small h6, h7 values
    // Start with identity-like homography and add small perspective
    let h6 = 0.00001;
    let h7 = 0.000005;
    let dx = 25.0;
    let dy = -18.0;

    let homography_params = [1.0, 0.0, dx, 0.0, 1.0, dy, h6, h7];
    let target_stars = apply_homography(&ref_stars, homography_params);

    let config = Config {
        transform_type: TransformType::Homography,
        min_stars: 8,
        min_matches: 6,
        max_sigma: 1.0,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    assert_eq!(result.transform.transform_type, TransformType::Homography);

    // Validate transformation accuracy
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(*ref_star);
        let error = t.distance(*target_star);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 2.0,
        "Max transformation error too large: {} pixels",
        max_error
    );

    assert!(
        result.rms_error < 1.5,
        "RMS error too large: {}",
        result.rms_error
    );
}

#[test]
fn test_registration_homography_with_rotation() {
    // Homography combining rotation and perspective
    let ref_stars = generate_random_positions(120, 2000.0, 2000.0, 77778);

    let angle_rad = 0.5_f64.to_radians();
    let cos_a = angle_rad.cos();
    let sin_a = angle_rad.sin();
    let dx = 35.0;
    let dy = -25.0;
    let h6 = 0.000008;
    let h7 = 0.000003;

    let homography_params = [cos_a, -sin_a, dx, sin_a, cos_a, dy, h6, h7];
    let target_stars = apply_homography(&ref_stars, homography_params);

    let config = Config {
        transform_type: TransformType::Homography,
        min_stars: 8,
        min_matches: 6,
        max_sigma: 1.0,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    assert_eq!(result.transform.transform_type, TransformType::Homography);

    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(*ref_star);
        let error = t.distance(*target_star);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 2.0,
        "Max transformation error too large: {} pixels",
        max_error
    );
}

// ============================================================================
// Cross-type validation tests
// ============================================================================

#[test]
fn test_transform_type_min_points() {
    // Verify min_points requirements for each transform type
    assert_eq!(TransformType::Translation.min_points(), 1);
    assert_eq!(TransformType::Euclidean.min_points(), 2);
    assert_eq!(TransformType::Similarity.min_points(), 2);
    assert_eq!(TransformType::Affine.min_points(), 3);
    assert_eq!(TransformType::Homography.min_points(), 4);
}

#[test]
fn test_transform_type_degrees_of_freedom() {
    assert_eq!(TransformType::Translation.degrees_of_freedom(), 2);
    assert_eq!(TransformType::Euclidean.degrees_of_freedom(), 3);
    assert_eq!(TransformType::Similarity.degrees_of_freedom(), 4);
    assert_eq!(TransformType::Affine.degrees_of_freedom(), 6);
    assert_eq!(TransformType::Homography.degrees_of_freedom(), 8);
}

#[test]
fn test_similarity_recovers_from_euclidean_data() {
    // When data only has rotation (no scale), Similarity should still work
    // and recover scale ≈ 1.0
    let ref_stars = generate_random_positions(60, 1500.0, 1500.0, 88888);

    let angle_rad = 0.6_f64.to_radians();
    let dx = 20.0;
    let dy = 15.0;
    let center_x = 750.0;
    let center_y = 750.0;

    // Apply Euclidean transform (scale = 1.0)
    let target_stars = transform_stars(&ref_stars, dx, dy, angle_rad, 1.0, center_x, center_y);

    // Use Similarity estimator
    let config = Config {
        transform_type: TransformType::Similarity,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 0.67,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    // Scale should be recovered as ~1.0
    let recovered_scale = result.transform.scale_factor();
    assert!(
        (recovered_scale - 1.0).abs() < 0.001,
        "Scale should be ~1.0 for Euclidean data, got {}",
        recovered_scale
    );

    // Rotation should be accurate
    let recovered_angle = result.transform.rotation_angle();
    let rotation_error = (recovered_angle - angle_rad).abs();
    assert!(
        rotation_error < 0.01,
        "Rotation error: {} vs {}",
        recovered_angle,
        angle_rad
    );
}

#[test]
fn test_affine_recovers_from_similarity_data() {
    // When data only has similarity transform, Affine should recover it correctly
    let ref_stars = generate_random_positions(80, 2000.0, 2000.0, 99999);

    let angle_rad = 0.4_f64.to_radians();
    let scale = 1.005;
    let dx = 30.0;
    let dy = -20.0;
    let center_x = 1000.0;
    let center_y = 1000.0;

    let target_stars = transform_stars(&ref_stars, dx, dy, angle_rad, scale, center_x, center_y);

    let config = Config {
        transform_type: TransformType::Affine,
        min_stars: 6,
        min_matches: 4,
        max_sigma: 0.67,
        ..Default::default()
    };

    let result = register_positions(&ref_stars, &target_stars, &config)
        .expect("Registration should succeed");

    // Validate by applying transform
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let t = result.transform.apply(*ref_star);
        let error = t.distance(*target_star);
        max_error = max_error.max(error);
    }

    assert!(
        max_error < 0.5,
        "Max transformation error too large: {} pixels",
        max_error
    );
}
