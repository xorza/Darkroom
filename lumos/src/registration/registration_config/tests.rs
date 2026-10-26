use glam::DVec2;

use crate::registration::registration_config::*;
use crate::registration::transform::TransformType;
use crate::registration::transform::linear_model::LinearModel;

#[test]
fn config_default_values() {
    let config = RegistrationConfig::default();
    assert!(matches!(config.model, TransformModel::Auto { .. }));
    assert_eq!(config.matching.max_stars, 200);
    assert_eq!(config.matching.min_stars, None);
    // Auto gates like Homography: 2 × 4 minimal points = 8.
    assert_eq!(config.matching.required_stars(config.model), 8);
    assert_eq!(
        config
            .matching
            .required_stars(TransformModel::Fixed(TransformType::Similarity)),
        4
    );
    assert_eq!(
        RegistrationMatchingConfig {
            min_stars: Some(20),
            ..Default::default()
        }
        .required_stars(TransformModel::default()),
        20
    );
    assert_eq!(config.matching.min_matches, 8);
    assert_eq!(config.matching.triangle.ratio_tolerance, 0.01);
    assert_eq!(config.matching.triangle.min_votes, 3);
    assert_eq!(config.ransac.max_iterations, 2000);
    assert_eq!(config.ransac.confidence, 0.995);
    assert_eq!(config.ransac.min_inlier_ratio, 0.3);
    assert_eq!(config.ransac.seed, 0);
    assert!(config.ransac.max_rotation.is_none());
    assert_eq!(config.ransac.local_max_iterations, 10);
    assert_eq!(config.max_rms_error, 2.0);
    assert!(config.model.sip().is_none());
    config.validate().unwrap();
}

#[test]
fn config_fast_preset() {
    let config = RegistrationConfig::fast();
    assert_eq!(config.ransac.max_iterations, 500);
    assert_eq!(config.matching.max_stars, 100);
    assert_eq!(config.ransac.local_max_iterations, 0);
    config.validate().unwrap();
}

#[test]
fn config_precise_preset() {
    let config = RegistrationConfig::precise();
    assert_eq!(config.ransac.max_iterations, 5000);
    assert_eq!(config.ransac.confidence, 0.999);
    assert!(config.model.sip().is_some());
    assert_eq!(config.max_rms_error, 1.0);
    config.validate().unwrap();
}

/// The wide-field preset corrects distortion by SIP under the model GRIC chooses. A fixed
/// homography under SIP has no `TransformModel` to say it: the correction sits under a
/// `LinearModel`, up to affine, as a homography's perspective terms are its quadratic ones.
#[test]
fn config_wide_field_preset() {
    let config = RegistrationConfig::wide_field();
    assert!(matches!(config.model, TransformModel::Auto { .. }));
    assert!(config.model.sip().is_some());
    assert!(config.ransac.max_rotation.is_none());
    assert!(config.ransac.scale_range.is_none());
    config.validate().unwrap();
    assert_eq!(
        LinearModel::try_from(TransformType::Homography),
        Err(TransformType::Homography)
    );
    // Auto under SIP climbs to affine, not homography, and sizes its gates there.
    assert_eq!(config.model.most_general(), TransformType::Affine);
}

#[test]
fn config_precise_wide_field_preset() {
    let config = RegistrationConfig::precise_wide_field();
    assert!(matches!(config.model, TransformModel::Auto { .. }));
    assert_eq!(config.matching.max_stars, 500);
    assert_eq!(config.matching.min_matches, 20);
    assert_eq!(config.matching.triangle.ratio_tolerance, 0.02);
    assert_eq!(config.ransac.max_iterations, 5000);
    assert_eq!(config.ransac.confidence, 0.9999);
    assert!(config.model.sip().is_some());
    assert_eq!(config.max_rms_error, 1.0);
    // Inherits unlimited rotation/scale from wide_field()
    assert!(config.ransac.max_rotation.is_none());
    assert!(config.ransac.scale_range.is_none());
    config.validate().unwrap();
}

#[test]
fn config_mosaic_preset() {
    let config = RegistrationConfig::mosaic();
    assert!(config.ransac.max_rotation.is_none());
    assert_eq!(
        config.ransac.scale_range,
        Some(ScaleRange { min: 0.5, max: 2.0 })
    );
    config.validate().unwrap();
}

#[test]
fn config_validation_rejects_invalid() {
    // Each case: a single out-of-range field and the field name its error must name.
    let cases: &[(RegistrationConfig, &str)] = &[
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    max_iterations: 0,
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac max_iterations",
        ),
        (
            RegistrationConfig {
                matching: RegistrationMatchingConfig {
                    max_stars: 2,
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "max_stars",
        ),
        (
            RegistrationConfig {
                matching: RegistrationMatchingConfig {
                    min_stars: Some(2),
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "min_stars",
        ),
        (
            RegistrationConfig {
                matching: RegistrationMatchingConfig {
                    max_stars: 5,
                    min_stars: Some(10),
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "max_stars",
        ),
        (
            RegistrationConfig {
                matching: RegistrationMatchingConfig {
                    triangle: TriangleConfig {
                        ratio_tolerance: 0.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ratio_tolerance",
        ),
        (
            RegistrationConfig {
                matching: RegistrationMatchingConfig {
                    triangle: TriangleConfig {
                        ratio_tolerance: 1.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ratio_tolerance",
        ),
        (
            RegistrationConfig {
                matching: RegistrationMatchingConfig {
                    triangle: TriangleConfig {
                        min_votes: 0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "min_votes",
        ),
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    confidence: 1.5,
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac confidence",
        ),
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    confidence: 0.0,
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac confidence",
        ),
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    confidence: 1.0,
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac confidence",
        ),
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    min_inlier_ratio: 0.0,
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac min_inlier_ratio",
        ),
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    max_rotation: Some(-0.1),
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac max_rotation",
        ),
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    max_rotation: Some(f64::NAN),
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac max_rotation",
        ),
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    scale_range: Some(ScaleRange { min: 1.5, max: 0.5 }),
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac scale_range maximum",
        ),
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    scale_range: Some(ScaleRange {
                        min: 0.8,
                        max: f64::INFINITY,
                    }),
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac scale_range maximum",
        ),
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    scale_range: Some(ScaleRange {
                        min: f64::INFINITY,
                        max: 1.2,
                    }),
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac scale_range minimum",
        ),
        (
            RegistrationConfig {
                max_rms_error: 0.0,
                ..RegistrationConfig::default()
            },
            "max_rms_error",
        ),
        (
            RegistrationConfig {
                max_rms_error: f64::INFINITY,
                ..RegistrationConfig::default()
            },
            "max_rms_error",
        ),
        (
            RegistrationConfig {
                model: TransformModel::Auto {
                    sip: Some(SipConfig {
                        reference_point: Some(DVec2::new(0.0, f64::NEG_INFINITY)),
                        ..Default::default()
                    }),
                },
                ..RegistrationConfig::default()
            },
            "SIP reference_point y",
        ),
        (
            // Homography needs 4 points, so min_matches = 3 is too few.
            RegistrationConfig {
                model: TransformModel::Fixed(TransformType::Homography),
                matching: RegistrationMatchingConfig {
                    min_matches: 3,
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "min_matches",
        ),
    ];

    for (config, expected) in cases {
        assert_eq!(&config.validate().unwrap_err().field, expected);
    }
}
