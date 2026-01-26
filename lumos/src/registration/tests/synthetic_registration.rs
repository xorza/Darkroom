//! Tests for registration using synthetic star fields.

use crate::registration::{RegistrationConfig, Registrator};

/// Generate a synthetic star field with random positions.
/// Returns a list of (x, y) coordinates.
fn generate_star_field(num_stars: usize, width: f64, height: f64, seed: u64) -> Vec<(f64, f64)> {
    // Simple LCG random number generator for reproducibility
    let mut state = seed;
    let mut next_random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as f64 / (1u64 << 31) as f64
    };

    let margin = 50.0;
    let mut stars = Vec::with_capacity(num_stars);

    for _ in 0..num_stars {
        let x = margin + next_random() * (width - 2.0 * margin);
        let y = margin + next_random() * (height - 2.0 * margin);
        stars.push((x, y));
    }

    stars
}

/// Apply a translation transform to a star field.
fn translate_stars(stars: &[(f64, f64)], dx: f64, dy: f64) -> Vec<(f64, f64)> {
    stars.iter().map(|(x, y)| (x + dx, y + dy)).collect()
}

/// Apply a similarity transform (translation + rotation + scale) to a star field.
fn transform_stars(
    stars: &[(f64, f64)],
    dx: f64,
    dy: f64,
    angle_rad: f64,
    scale: f64,
    center_x: f64,
    center_y: f64,
) -> Vec<(f64, f64)> {
    let cos_a = angle_rad.cos() * scale;
    let sin_a = angle_rad.sin() * scale;

    stars
        .iter()
        .map(|(x, y)| {
            // Translate to origin, rotate+scale, translate back, then apply offset
            let rx = x - center_x;
            let ry = y - center_y;
            let new_x = cos_a * rx - sin_a * ry + center_x + dx;
            let new_y = sin_a * rx + cos_a * ry + center_y + dy;
            (new_x, new_y)
        })
        .collect()
}

#[test]
fn test_registration_translation_only() {
    // Generate reference star field
    let ref_stars = generate_star_field(50, 1000.0, 1000.0, 12345);

    // Apply known translation
    let dx = 25.5;
    let dy = -15.3;
    let target_stars = translate_stars(&ref_stars, dx, dy);

    // Configure registration for translation only
    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(2.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    // Extract recovered translation
    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    // Validate translation (should be very close to original)
    let dx_error = (recovered_dx - dx).abs();
    let dy_error = (recovered_dy - dy).abs();

    assert!(
        dx_error < 0.1,
        "X translation error too large: expected {}, got {}, error {}",
        dx,
        recovered_dx,
        dx_error
    );
    assert!(
        dy_error < 0.1,
        "Y translation error too large: expected {}, got {}, error {}",
        dy,
        recovered_dy,
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
    let ref_stars = generate_star_field(80, 2000.0, 2000.0, 54321);

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
    let config = RegistrationConfig::builder()
        .with_scale()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    // Validate by applying the recovered transform to reference stars
    // and checking they match target stars
    let mut max_error = 0.0f64;
    for (ref_star, target_star) in ref_stars.iter().zip(target_stars.iter()) {
        let (transformed_x, transformed_y) = result.transform.apply(ref_star.0, ref_star.1);
        let error = ((transformed_x - target_star.0).powi(2)
            + (transformed_y - target_star.1).powi(2))
        .sqrt();
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
    let ref_stars = generate_star_field(100, 1500.0, 1500.0, 99999);

    // Apply translation with some noise in star positions
    let dx = 50.0;
    let dy = 35.0;

    let mut state = 11111u64;
    let mut next_random = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((state >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0 // -1 to 1
    };

    let target_stars: Vec<(f64, f64)> = ref_stars
        .iter()
        .map(|(x, y)| {
            let noise_x = next_random() * 0.5; // +/- 0.5 pixel noise
            let noise_y = next_random() * 0.5;
            (x + dx + noise_x, y + dy + noise_y)
        })
        .collect();

    // Configure registration
    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(3.0) // Allow more residual due to noise
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    // Extract recovered translation
    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    // Validate translation (allow more error due to noise)
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
    let ref_stars = generate_star_field(60, 2000.0, 2000.0, 77777);

    // Apply large translation (simulating significant pointing offset)
    let dx = 200.0;
    let dy = -150.0;
    let target_stars = translate_stars(&ref_stars, dx, dy);

    // Configure registration
    let config = RegistrationConfig::builder()
        .translation_only()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(2.0)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
        .expect("Registration should succeed");

    let (recovered_dx, recovered_dy) = result.transform.translation_components();

    let dx_error = (recovered_dx - dx).abs();
    let dy_error = (recovered_dy - dy).abs();

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
    let ref_stars = generate_star_field(50, 1000.0, 1000.0, 12345);
    let target_stars = translate_stars(&ref_stars, 10.0, -5.0);

    let config = RegistrationConfig::builder()
        .with_scale()
        .min_stars(6)
        .min_matched_stars(4)
        .build();

    let registrator = Registrator::new(config);
    let result = registrator
        .register_stars(&ref_stars, &target_stars)
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
