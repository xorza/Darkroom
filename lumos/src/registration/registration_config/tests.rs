use glam::DVec2;
use std::num::NonZeroUsize;

use crate::registration::registration_config::*;
use crate::registration::transform::TransformType;
use crate::registration::transform::linear_model::LinearModel;

#[test]
fn config_default_values() {
    let config = RegistrationConfig::default();
    assert!(matches!(
        config.model,
        TransformModel::Plain(ModelChoice::Auto)
    ));
    assert_eq!(config.matching.max_stars, 200);
    // Auto gates like Homography: 2 × 4 minimal points = 8; under SIP like Affine, 2 × 3 = 6; a
    // similarity 2 × 2 = 4; a translation's 2 × 1 = 2 is floored at triangle matching's 3.
    assert_eq!(config.model.required_stars(), 8);
    let fixed = |transform_type| TransformModel::Plain(ModelChoice::Fixed(transform_type));
    assert_eq!(fixed(TransformType::Similarity).required_stars(), 4);
    assert_eq!(fixed(TransformType::Translation).required_stars(), 3);
    assert_eq!(
        TransformModel::WithSip(ModelChoice::Auto, SipConfig::default()).required_stars(),
        6
    );
    assert_eq!(config.matching.min_matches, 8);
    assert_eq!(config.matching.triangle.ratio_tolerance, 0.01);
    assert_eq!(config.matching.triangle.min_votes, 3);
    assert_eq!(config.ransac.max_iterations, 2000);
    assert_eq!(config.ransac.confidence, 0.995);
    assert_eq!(config.ransac.min_inlier_ratio, 0.3);
    assert_eq!(config.ransac.seed, 0);
    assert!(config.ransac.max_rotation_rad.is_none());
    assert_eq!(config.ransac.local_optimization, NonZeroUsize::new(10));
    assert_eq!(config.max_rms_error_px, 2.0);
    assert!(config.model.sip().is_none());
    config.validate().unwrap();
}

#[test]
fn config_fast_preset() {
    let config = RegistrationConfig::fast();
    assert_eq!(config.ransac.max_iterations, 500);
    assert_eq!(config.matching.max_stars, 100);
    assert_eq!(config.ransac.local_optimization, None);
    config.validate().unwrap();
}

#[test]
fn config_precise_preset() {
    let config = RegistrationConfig::precise();
    assert_eq!(config.ransac.max_iterations, 5000);
    assert_eq!(config.ransac.confidence, 0.999);
    assert!(config.model.sip().is_some());
    assert_eq!(config.max_rms_error_px, 1.0);
    config.validate().unwrap();
}

/// The wide-field preset corrects distortion by SIP under the model GRIC chooses. A fixed
/// homography under SIP has no `TransformModel` to say it: the correction sits under a
/// `LinearModel`, up to affine, as a homography's perspective terms are its quadratic ones.
#[test]
fn config_wide_field_preset() {
    let config = RegistrationConfig::wide_field();
    assert_eq!(
        config.model,
        TransformModel::WithSip(ModelChoice::Auto, SipConfig::default())
    );
    assert!(config.ransac.max_rotation_rad.is_none());
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
    assert_eq!(
        config.model,
        TransformModel::WithSip(ModelChoice::Auto, SipConfig::default())
    );
    assert_eq!(config.matching.max_stars, 500);
    assert_eq!(config.matching.min_matches, 20);
    assert_eq!(config.matching.triangle.ratio_tolerance, 0.02);
    assert_eq!(config.ransac.max_iterations, 5000);
    assert_eq!(config.ransac.confidence, 0.9999);
    assert!(config.model.sip().is_some());
    assert_eq!(config.max_rms_error_px, 1.0);
    // Inherits unlimited rotation/scale from wide_field()
    assert!(config.ransac.max_rotation_rad.is_none());
    assert!(config.ransac.scale_range.is_none());
    config.validate().unwrap();
}

#[test]
fn config_wide_scale_preset() {
    RegistrationConfig {
        matching: RegistrationMatchingConfig {
            max_stars: MAX_STARS,
            ..Default::default()
        },
        ..RegistrationConfig::default()
    }
    .validate()
    .unwrap();
    let config = RegistrationConfig::wide_scale();
    assert!(config.ransac.max_rotation_rad.is_none());
    assert_eq!(config.ransac.scale_range, Some(ScaleRange::new(0.5, 2.0)));
    config.validate().unwrap();
}

/// A scale range is a finite, positive minimum below a finite maximum, and refused otherwise by
/// the end that breaks it; its ends are kept as given.
#[test]
fn a_scale_range_is_checked_where_it_is_made() {
    let range = ScaleRange::checked(0.5, 2.0).unwrap();
    assert_eq!((range.min(), range.max()), (0.5, 2.0));
    for (min, max, field) in [
        (1.5, 0.5, "ransac scale_range maximum"),
        (0.8, 0.8, "ransac scale_range maximum"),
        (0.8, f64::INFINITY, "ransac scale_range maximum"),
        (f64::INFINITY, 1.2, "ransac scale_range minimum"),
        (0.0, 1.2, "ransac scale_range minimum"),
        (f64::NAN, 1.2, "ransac scale_range minimum"),
    ] {
        assert_eq!(
            ScaleRange::checked(min, max).unwrap_err().field,
            field,
            "{min} to {max}"
        );
    }
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
                    max_stars: MAX_STARS + 1,
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "max_stars",
        ),
        (
            RegistrationConfig {
                matching: RegistrationMatchingConfig {
                    max_stars: 5,
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
                    max_rotation_rad: Some(-0.1),
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac max_rotation_rad",
        ),
        (
            RegistrationConfig {
                ransac: RansacConfig {
                    max_rotation_rad: Some(f64::NAN),
                    ..Default::default()
                },
                ..RegistrationConfig::default()
            },
            "ransac max_rotation_rad",
        ),
        (
            RegistrationConfig {
                max_rms_error_px: 0.0,
                ..RegistrationConfig::default()
            },
            "max_rms_error_px",
        ),
        (
            RegistrationConfig {
                max_rms_error_px: f64::INFINITY,
                ..RegistrationConfig::default()
            },
            "max_rms_error_px",
        ),
        (
            RegistrationConfig {
                model: TransformModel::WithSip(
                    ModelChoice::Auto,
                    SipConfig {
                        reference_point: Some(DVec2::new(0.0, f64::NEG_INFINITY)),
                        ..Default::default()
                    },
                ),
                ..RegistrationConfig::default()
            },
            "SIP reference_point y",
        ),
        (
            // Homography needs 4 points, so min_matches = 3 is too few.
            RegistrationConfig {
                model: TransformModel::Plain(ModelChoice::Fixed(TransformType::Homography)),
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
