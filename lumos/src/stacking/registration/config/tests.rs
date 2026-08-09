use crate::stacking::registration::config::*;
use crate::testing::prelude::*;

#[test]
fn config_default_values() {
    let config = Config::default();
    assert_eq!(config.transform_type, TransformType::Auto);
    assert_eq!(config.matching.max_stars, 200);
    assert_eq!(config.matching.min_stars, None);
    // Auto gates like Homography: 2 × 4 minimal points = 8.
    assert_eq!(config.matching.required_stars(config.transform_type), 8);
    assert_eq!(config.matching.required_stars(TransformType::Similarity), 4);
    assert_eq!(
        RegistrationMatchingConfig {
            min_stars: Some(20),
            ..Default::default()
        }
        .required_stars(TransformType::Auto),
        20
    );
    assert_eq!(config.matching.min_matches, 8);
    assert!((config.matching.triangle.ratio_tolerance - 0.01).abs() < 1e-10);
    assert_eq!(config.matching.triangle.min_votes, 3);
    assert!(config.matching.triangle.check_orientation);
    assert_eq!(config.ransac.max_iterations, 2000);
    assert!((config.ransac.confidence - 0.995).abs() < 1e-10);
    assert!((config.ransac.min_inlier_ratio - 0.3).abs() < 1e-10);
    assert!(config.ransac.seed.is_none());
    assert!(config.ransac.local_optimization);
    assert_eq!(config.ransac.lo_iterations, 10);
    assert!((config.max_rms_error - 2.0).abs() < 1e-10);
    assert!(config.sip.is_none());
    assert_eq!(config.warp.method, InterpolationMethod::Lanczos3);
    assert!((config.warp.border_value - 0.0).abs() < 1e-10);
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
    assert!((config.ransac.confidence - 0.999).abs() < 1e-10);
    assert!(config.sip.is_some());
    assert!((config.max_rms_error - 1.0).abs() < 1e-10);
    config.validate().unwrap();
}

#[test]
fn config_wide_field_preset() {
    let config = Config::wide_field();
    assert_eq!(config.transform_type, TransformType::Homography);
    assert!(config.sip.is_some());
    assert!(config.ransac.max_rotation.is_none());
    assert!(config.ransac.scale_range.is_none());
    config.validate().unwrap();
}

#[test]
fn config_precise_wide_field_preset() {
    let config = Config::precise_wide_field();
    assert_eq!(config.transform_type, TransformType::Homography);
    assert_eq!(config.matching.max_stars, 500);
    assert_eq!(config.matching.min_matches, 20);
    assert!((config.matching.triangle.ratio_tolerance - 0.02).abs() < 1e-10);
    assert_eq!(config.ransac.max_iterations, 5000);
    assert!((config.ransac.confidence - 0.9999).abs() < 1e-10);
    assert!(config.sip.is_some());
    assert!((config.max_rms_error - 1.0).abs() < 1e-10);
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
fn config_custom() {
    let config = Config {
        transform_type: TransformType::Similarity,
        ransac: RansacConfig {
            max_iterations: 1000,
            ..Default::default()
        },
        ..Config::default()
    };
    assert_eq!(config.transform_type, TransformType::Similarity);
    assert_eq!(config.ransac.max_iterations, 1000);
    config.validate().unwrap();
}

#[test]
fn interpolation_method_kernel_radius() {
    assert_eq!(InterpolationMethod::Nearest.kernel_radius(), 1);
    assert_eq!(InterpolationMethod::Bilinear.kernel_radius(), 1);
    assert_eq!(InterpolationMethod::Bicubic.kernel_radius(), 2);
    assert_eq!(InterpolationMethod::Lanczos2.kernel_radius(), 2);
    assert_eq!(InterpolationMethod::Lanczos3.kernel_radius(), 3);
    assert_eq!(InterpolationMethod::Lanczos4.kernel_radius(), 4);
}

#[test]
fn test_lanczos_param() {
    // Non-Lanczos methods return None
    assert_eq!(InterpolationMethod::Nearest.lanczos_param(), None);
    assert_eq!(InterpolationMethod::Bilinear.lanczos_param(), None);
    assert_eq!(InterpolationMethod::Bicubic.lanczos_param(), None);
    // Lanczos methods return their parameter a
    assert_eq!(InterpolationMethod::Lanczos2.lanczos_param(), Some(2));
    assert_eq!(InterpolationMethod::Lanczos3.lanczos_param(), Some(3));
    assert_eq!(InterpolationMethod::Lanczos4.lanczos_param(), Some(4));
}

#[test]
fn interpolation_method_default() {
    let method = InterpolationMethod::default();
    assert_eq!(method, InterpolationMethod::Lanczos3);
}

#[test]
fn warp_params_defaults() {
    let default = WarpParams::default();
    assert_eq!(default.method, InterpolationMethod::Lanczos3);
    assert_eq!(default.border_value, 0.0);
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
                transform_type: TransformType::Homography,
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

#[test]
fn config_presets_differ() {
    // Verify presets produce different configs (parameter sensitivity)
    let default = Config::default();
    let fast = Config::fast();
    let precise = Config::precise();

    // fast has fewer iterations than default
    assert!(fast.ransac.max_iterations < default.ransac.max_iterations);
    // precise has more iterations than default
    assert!(precise.ransac.max_iterations > default.ransac.max_iterations);
    // precise has tighter RMS tolerance
    assert!(precise.max_rms_error < default.max_rms_error);
    // fast disables LO, default enables it
    assert!(!fast.ransac.local_optimization);
    assert!(default.ransac.local_optimization);
}

#[test]
fn config_all_presets_validate() {
    Config::default().validate().unwrap();
    Config::fast().validate().unwrap();
    Config::precise().validate().unwrap();
    Config::wide_field().validate().unwrap();
    Config::precise_wide_field().validate().unwrap();
    Config::mosaic().validate().unwrap();
}
