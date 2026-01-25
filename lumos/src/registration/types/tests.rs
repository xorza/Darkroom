//! Tests for core registration types.

use super::*;
use std::f64::consts::PI;

const EPSILON: f64 = 1e-10;

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < EPSILON
}

#[test]
fn test_identity_transform() {
    let t = TransformMatrix::identity();
    let (x, y) = t.apply(5.0, 7.0);
    assert!(approx_eq(x, 5.0));
    assert!(approx_eq(y, 7.0));
}

#[test]
fn test_translation_transform() {
    let t = TransformMatrix::translation(10.0, -5.0);
    let (x, y) = t.apply(3.0, 4.0);
    assert!(approx_eq(x, 13.0));
    assert!(approx_eq(y, -1.0));
}

#[test]
fn test_rotation_90_degrees() {
    let t = TransformMatrix::euclidean(0.0, 0.0, PI / 2.0);
    let (x, y) = t.apply(1.0, 0.0);
    assert!(approx_eq(x, 0.0));
    assert!(approx_eq(y, 1.0));
}

#[test]
fn test_rotation_180_degrees() {
    let t = TransformMatrix::euclidean(0.0, 0.0, PI);
    let (x, y) = t.apply(1.0, 0.0);
    assert!(approx_eq(x, -1.0));
    assert!(approx_eq(y, 0.0));
}

#[test]
fn test_scale_transform() {
    let t = TransformMatrix::similarity(0.0, 0.0, 0.0, 2.0);
    let (x, y) = t.apply(3.0, 4.0);
    assert!(approx_eq(x, 6.0));
    assert!(approx_eq(y, 8.0));
}

#[test]
fn test_similarity_with_rotation_and_scale() {
    let t = TransformMatrix::similarity(5.0, 10.0, PI / 2.0, 2.0);
    let (x, y) = t.apply(1.0, 0.0);
    // Rotate 90° then scale 2x: (1,0) -> (0,1) -> (0,2), then translate
    assert!(approx_eq(x, 5.0));
    assert!(approx_eq(y, 12.0));
}

#[test]
fn test_affine_transform() {
    // Shear transform
    let t = TransformMatrix::affine([1.0, 0.5, 0.0, 0.0, 1.0, 0.0]);
    let (x, y) = t.apply(2.0, 2.0);
    assert!(approx_eq(x, 3.0)); // 2 + 0.5*2
    assert!(approx_eq(y, 2.0));
}

#[test]
fn test_transform_inverse() {
    let t = TransformMatrix::similarity(10.0, -5.0, PI / 4.0, 1.5);
    let inv = t.inverse();

    let (x1, y1) = t.apply(3.0, 7.0);
    let (x2, y2) = inv.apply(x1, y1);

    assert!(approx_eq(x2, 3.0));
    assert!(approx_eq(y2, 7.0));
}

#[test]
fn test_transform_point_roundtrip() {
    let transforms = vec![
        TransformMatrix::translation(5.0, -3.0),
        TransformMatrix::euclidean(2.0, 3.0, 0.7),
        TransformMatrix::similarity(1.0, 2.0, -0.5, 1.3),
        TransformMatrix::affine([1.1, 0.2, 5.0, -0.1, 0.9, -3.0]),
    ];

    for t in transforms {
        let inv = t.inverse();
        for &(x, y) in &[(0.0, 0.0), (10.0, 10.0), (-5.0, 7.0), (100.0, -50.0)] {
            let (x1, y1) = t.apply(x, y);
            let (x2, y2) = inv.apply(x1, y1);
            assert!(
                approx_eq(x2, x) && approx_eq(y2, y),
                "Roundtrip failed for ({}, {}): got ({}, {})",
                x,
                y,
                x2,
                y2
            );
        }
    }
}

#[test]
fn test_compose_translations() {
    let t1 = TransformMatrix::translation(5.0, 3.0);
    let t2 = TransformMatrix::translation(2.0, -1.0);
    let composed = t1.compose(&t2);

    let (x, y) = composed.apply(0.0, 0.0);
    // t2 first: (0,0) -> (2,-1), then t1: (2,-1) -> (7,2)
    assert!(approx_eq(x, 7.0));
    assert!(approx_eq(y, 2.0));
}

#[test]
fn test_compose_rotations() {
    let t1 = TransformMatrix::euclidean(0.0, 0.0, PI / 4.0);
    let t2 = TransformMatrix::euclidean(0.0, 0.0, PI / 4.0);
    let composed = t1.compose(&t2);

    let (x, y) = composed.apply(1.0, 0.0);
    // Two 45° rotations = 90° rotation
    assert!(approx_eq(x, 0.0));
    assert!(approx_eq(y, 1.0));
}

#[test]
fn test_translation_components() {
    let t = TransformMatrix::translation(7.0, -3.0);
    let (tx, ty) = t.translation_components();
    assert!(approx_eq(tx, 7.0));
    assert!(approx_eq(ty, -3.0));
}

#[test]
fn test_rotation_angle() {
    let angle = 0.5;
    let t = TransformMatrix::euclidean(0.0, 0.0, angle);
    assert!(approx_eq(t.rotation_angle(), angle));
}

#[test]
fn test_scale_factor() {
    let scale = 2.5;
    let t = TransformMatrix::similarity(0.0, 0.0, 0.0, scale);
    assert!(approx_eq(t.scale_factor(), scale));
}

#[test]
fn test_is_valid() {
    let valid = TransformMatrix::similarity(1.0, 2.0, 0.5, 1.5);
    assert!(valid.is_valid());

    // Degenerate matrix (zero scale)
    let degenerate = TransformMatrix::from_matrix(
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        TransformType::Affine,
    );
    assert!(!degenerate.is_valid());
}

#[test]
fn test_config_default_values() {
    let config = RegistrationConfig::default();
    assert_eq!(config.transform_type, TransformType::Similarity);
    assert_eq!(config.ransac_iterations, 1000);
    assert!((config.ransac_threshold - 2.0).abs() < 1e-10);
}

#[test]
fn test_config_builder() {
    let config = RegistrationConfig::builder()
        .with_rotation()
        .ransac_iterations(500)
        .ransac_threshold(3.0)
        .max_stars(100)
        .build();

    assert_eq!(config.transform_type, TransformType::Euclidean);
    assert_eq!(config.ransac_iterations, 500);
    assert!((config.ransac_threshold - 3.0).abs() < 1e-10);
    assert_eq!(config.max_stars_for_matching, 100);
}

#[test]
fn test_config_validation() {
    // Valid config should not panic
    let config = RegistrationConfig::default();
    config.validate();
}

#[test]
#[should_panic(expected = "RANSAC iterations must be positive")]
fn test_config_invalid_iterations() {
    let config = RegistrationConfig {
        ransac_iterations: 0,
        ..Default::default()
    };
    config.validate();
}

#[test]
fn test_transform_type_min_points() {
    assert_eq!(TransformType::Translation.min_points(), 1);
    assert_eq!(TransformType::Euclidean.min_points(), 2);
    assert_eq!(TransformType::Similarity.min_points(), 2);
    assert_eq!(TransformType::Affine.min_points(), 3);
    assert_eq!(TransformType::Homography.min_points(), 4);
}

#[test]
fn test_registration_result_new() {
    let transform = TransformMatrix::translation(1.0, 2.0);
    let matches = vec![(0, 0), (1, 1), (2, 2)];
    let residuals = vec![0.1, 0.2, 0.15];

    let result = RegistrationResult::new(transform, matches, residuals);

    assert_eq!(result.num_inliers, 3);
    assert!(result.rms_error > 0.0);
    assert!(result.max_error > 0.0);
    assert!(result.quality_score >= 0.0 && result.quality_score <= 1.0);
}

#[test]
fn test_star_match() {
    let m = StarMatch {
        ref_idx: 5,
        target_idx: 10,
        votes: 3,
        confidence: 0.95,
    };
    assert_eq!(m.ref_idx, 5);
    assert_eq!(m.target_idx, 10);
    assert_eq!(m.votes, 3);
}

#[test]
fn test_registration_error_display() {
    let err = RegistrationError::InsufficientStars {
        found: 5,
        required: 10,
    };
    let msg = format!("{}", err);
    assert!(msg.contains("5"));
    assert!(msg.contains("10"));
}

#[test]
fn test_homography_transform() {
    // Simple homography that acts like translation
    let t = TransformMatrix::homography([1.0, 0.0, 5.0, 0.0, 1.0, 3.0, 0.0, 0.0]);
    let (x, y) = t.apply(2.0, 4.0);
    assert!(approx_eq(x, 7.0));
    assert!(approx_eq(y, 7.0));
}

#[test]
fn test_homography_perspective() {
    // Homography with perspective component
    let t = TransformMatrix::homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.001, 0.0]);
    let (x, y) = t.apply(100.0, 0.0);
    // w = 0.001 * 100 + 1 = 1.1
    // x' = 100 / 1.1 ≈ 90.9
    assert!((x - 90.909).abs() < 0.01);
    assert!(approx_eq(y, 0.0));
}
