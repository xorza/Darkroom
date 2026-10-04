use crate::internals::prelude::*;
use crate::registration::config::*;

#[test]
fn config_default_values() {
    let config = Config::default();
    assert_eq!(config.transform_type, TransformModel::Auto);
    assert_eq!(config.matching.max_stars, 200);
    assert_eq!(config.matching.min_stars, None);
    // Auto gates like Homography: 2 × 4 minimal points = 8.
    assert_eq!(config.matching.required_stars(config.transform_type), 8);
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
        .required_stars(TransformModel::Auto),
        20
    );
    assert_eq!(config.matching.min_matches, 8);
    assert_eq!(config.matching.triangle.ratio_tolerance, 0.01);
    assert_eq!(config.matching.triangle.min_votes, 3);
    assert!(config.matching.triangle.check_orientation);
    assert_eq!(config.ransac.max_iterations, 2000);
    assert_eq!(config.ransac.confidence, 0.995);
    assert_eq!(config.ransac.min_inlier_ratio, 0.3);
    assert_eq!(config.ransac.seed, 0);
    assert!(config.ransac.max_rotation.is_none());
    assert!(config.ransac.local_optimization);
    assert_eq!(config.ransac.lo_iterations, 10);
    assert_eq!(config.max_rms_error, 2.0);
    assert!(config.sip.is_none());
    assert_eq!(config.warp.method, InterpolationMethod::Lanczos3);
    assert_eq!(config.warp.border_value, 0.0);
    config.validate().unwrap();
}

#[test]
fn config_fast_preset() {
    let config = Config::fast();
    assert_eq!(config.ransac.max_iterations, 500);
    assert_eq!(config.matching.max_stars, 100);
    assert!(!config.ransac.local_optimization);
    assert_eq!(config.warp.method, InterpolationMethod::Bilinear);
    config.validate().unwrap();
}

#[test]
fn config_precise_preset() {
    let config = Config::precise();
    assert_eq!(config.ransac.max_iterations, 5000);
    assert_eq!(config.ransac.confidence, 0.999);
    assert!(config.sip.is_some());
    assert_eq!(config.max_rms_error, 1.0);
    config.validate().unwrap();
}

/// The wide-field preset corrects distortion by SIP under the model GRIC chooses; a fixed
/// homography under SIP is refused, its perspective terms being the correction's quadratic ones.
#[test]
fn config_wide_field_preset() {
    let config = Config::wide_field();
    assert_eq!(config.transform_type, TransformModel::Auto);
    assert!(config.sip.is_some());
    assert!(config.ransac.max_rotation.is_none());
    assert!(config.ransac.scale_range.is_none());
    config.validate().unwrap();
    let homography = Config {
        transform_type: TransformModel::Fixed(TransformType::Homography),
        ..config
    };
    assert_eq!(
        homography.validate().unwrap_err().field,
        "transform_type with SIP"
    );
}

#[test]
fn config_precise_wide_field_preset() {
    let config = Config::precise_wide_field();
    assert_eq!(config.transform_type, TransformModel::Auto);
    assert_eq!(config.matching.max_stars, 500);
    assert_eq!(config.matching.min_matches, 20);
    assert_eq!(config.matching.triangle.ratio_tolerance, 0.02);
    assert_eq!(config.ransac.max_iterations, 5000);
    assert_eq!(config.ransac.confidence, 0.9999);
    assert!(config.sip.is_some());
    assert_eq!(config.max_rms_error, 1.0);
    // Inherits unlimited rotation/scale from wide_field()
    assert!(config.ransac.max_rotation.is_none());
    assert!(config.ransac.scale_range.is_none());
    config.validate().unwrap();
}

#[test]
fn config_mosaic_preset() {
    let config = Config::mosaic();
    assert!(config.ransac.max_rotation.is_none());
    assert_eq!(config.ransac.scale_range, Some((0.5, 2.0)));
    config.validate().unwrap();
}

#[test]
fn warp_params_defaults() {
    let default = WarpParams::default();
    assert_eq!(default.method, InterpolationMethod::Lanczos3);
    assert_eq!(default.border_value, 0.0);
    assert_eq!(default.clamping_threshold, Some(0.3));
    // Both ends of the range are thresholds PixInsight accepts, and `None` is the clamp off.
    for clamping_threshold in [Some(0.0), Some(1.0), None] {
        Config {
            warp: WarpParams {
                clamping_threshold,
                ..Default::default()
            },
            ..Config::default()
        }
        .validate()
        .unwrap();
    }
}

#[test]
fn config_validation_rejects_invalid() {
    // Each case: a single out-of-range field and the field name its error must name.
    let cases: &[(Config, &str)] = &[
        (
            Config {
                ransac: RansacConfig {
                    max_iterations: 0,
                    ..Default::default()
                },
                ..Config::default()
            },
            "ransac max_iterations",
        ),
        (
            Config {
                matching: RegistrationMatchingConfig {
                    max_stars: 2,
                    ..Default::default()
                },
                ..Config::default()
            },
            "max_stars",
        ),
        (
            Config {
                matching: RegistrationMatchingConfig {
                    min_stars: Some(2),
                    ..Default::default()
                },
                ..Config::default()
            },
            "min_stars",
        ),
        (
            Config {
                matching: RegistrationMatchingConfig {
                    max_stars: 5,
                    min_stars: Some(10),
                    ..Default::default()
                },
                ..Config::default()
            },
            "max_stars",
        ),
        (
            Config {
                matching: RegistrationMatchingConfig {
                    triangle: TriangleConfig {
                        ratio_tolerance: 0.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Config::default()
            },
            "ratio_tolerance",
        ),
        (
            Config {
                matching: RegistrationMatchingConfig {
                    triangle: TriangleConfig {
                        ratio_tolerance: 1.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Config::default()
            },
            "ratio_tolerance",
        ),
        (
            Config {
                matching: RegistrationMatchingConfig {
                    triangle: TriangleConfig {
                        min_votes: 0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Config::default()
            },
            "min_votes",
        ),
        (
            Config {
                ransac: RansacConfig {
                    confidence: 1.5,
                    ..Default::default()
                },
                ..Config::default()
            },
            "ransac confidence",
        ),
        (
            Config {
                ransac: RansacConfig {
                    min_inlier_ratio: 0.0,
                    ..Default::default()
                },
                ..Config::default()
            },
            "ransac min_inlier_ratio",
        ),
        (
            Config {
                ransac: RansacConfig {
                    local_optimization: true,
                    lo_iterations: 0,
                    ..Default::default()
                },
                ..Config::default()
            },
            "ransac lo_iterations",
        ),
        (
            Config {
                ransac: RansacConfig {
                    max_rotation: Some(-0.1),
                    ..Default::default()
                },
                ..Config::default()
            },
            "ransac max_rotation",
        ),
        (
            Config {
                ransac: RansacConfig {
                    max_rotation: Some(f64::NAN),
                    ..Default::default()
                },
                ..Config::default()
            },
            "ransac max_rotation",
        ),
        (
            Config {
                ransac: RansacConfig {
                    scale_range: Some((1.5, 0.5)),
                    ..Default::default()
                },
                ..Config::default()
            },
            "ransac scale_range maximum",
        ),
        (
            Config {
                ransac: RansacConfig {
                    scale_range: Some((0.8, f64::INFINITY)),
                    ..Default::default()
                },
                ..Config::default()
            },
            "ransac scale_range maximum",
        ),
        (
            Config {
                ransac: RansacConfig {
                    scale_range: Some((f64::INFINITY, 1.2)),
                    ..Default::default()
                },
                ..Config::default()
            },
            "ransac scale_range minimum",
        ),
        (
            Config {
                max_rms_error: 0.0,
                ..Config::default()
            },
            "max_rms_error",
        ),
        (
            Config {
                max_rms_error: f64::INFINITY,
                ..Config::default()
            },
            "max_rms_error",
        ),
        (
            Config {
                sip: Some(SipConfig {
                    order: 6,
                    ..Default::default()
                }),
                ..Config::default()
            },
            "SIP order",
        ),
        (
            Config {
                sip: Some(SipConfig {
                    reference_point: Some(DVec2::new(0.0, f64::NEG_INFINITY)),
                    ..Default::default()
                }),
                ..Config::default()
            },
            "SIP reference_point y",
        ),
        (
            // Homography needs 4 points, so min_matches = 3 is too few.
            Config {
                transform_type: TransformModel::Fixed(TransformType::Homography),
                matching: RegistrationMatchingConfig {
                    min_matches: 3,
                    ..Default::default()
                },
                ..Config::default()
            },
            "min_matches",
        ),
        (
            Config {
                warp: WarpParams {
                    border_value: f32::NAN,
                    ..Default::default()
                },
                ..Config::default()
            },
            "warp border_value",
        ),
        (
            Config {
                warp: WarpParams {
                    clamping_threshold: Some(1.5),
                    ..Default::default()
                },
                ..Config::default()
            },
            "warp clamping_threshold",
        ),
        (
            Config {
                warp: WarpParams {
                    clamping_threshold: Some(f32::NAN),
                    ..Default::default()
                },
                ..Config::default()
            },
            "warp clamping_threshold",
        ),
    ];

    for (config, expected) in cases {
        assert_eq!(&config.validate().unwrap_err().field, expected);
    }
}

#[test]
fn config_lo_iterations_zero_ok_when_lo_disabled() {
    // lo_iterations is only validated when local_optimization is enabled.
    let config = Config {
        ransac: RansacConfig {
            local_optimization: false,
            lo_iterations: 0,
            ..Default::default()
        },
        ..Config::default()
    };
    assert!(config.validate().is_ok());
}
