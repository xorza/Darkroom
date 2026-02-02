//! Tests for core registration types.

use super::*;
use crate::registration::pipeline::{RegistrationConfig, RegistrationError, RegistrationResult};
use crate::registration::transform::{TransformMatrix, TransformType};

#[test]
fn test_config_default_values() {
    let config = RegistrationConfig::default();
    assert_eq!(config.transform_type, TransformType::Similarity);
    assert_eq!(config.ransac_iterations, 1000);
    assert!((config.ransac_threshold - 2.0).abs() < 1e-10);
}

#[test]
fn test_config_struct_init() {
    let config = RegistrationConfig {
        transform_type: TransformType::Euclidean,
        ransac_iterations: 500,
        ransac_threshold: 3.0,
        max_stars_for_matching: 100,
        ..Default::default()
    };

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
