use crate::stacking::star_detection::config::detection_config::MAX_DEBLEND_N_THRESHOLDS;
use crate::stacking::star_detection::config::measurement_config::NoiseModel;
use crate::stacking::star_detection::config::*;

fn multi_threshold(n_thresholds: usize) -> Deblend {
    Deblend::MultiThreshold {
        n_thresholds,
        min_contrast: 0.005,
    }
}

fn configured(update: impl FnOnce(&mut Config)) -> Config {
    let mut config = Config::default();
    update(&mut config);
    config
}

#[test]
fn noise_model_uses_normalized_signal_units() {
    let model = NoiseModel::from_normalized(1_000.0, 10.0);
    assert_eq!(model.electrons_per_normalized_unit, 1_000.0);
    assert_eq!(model.read_noise_electrons, 10.0);
    assert_eq!(model.validate(), Ok(()));

    // 2/1000 + 4 × (0.02² + (10/1000)²) = 0.004 normalized².
    let variance = model.variance_normalized(2.0, 0.02, 4);
    assert!((variance - 0.004).abs() < 1e-12);
}

#[test]
fn noise_model_invalid_parameters_return_exact_errors() {
    let cases = [
        (
            NoiseModel::from_normalized(0.0, 5.0),
            "electrons_per_normalized_unit",
            0.0,
        ),
        (
            NoiseModel::from_normalized(f32::INFINITY, 5.0),
            "electrons_per_normalized_unit",
            f64::INFINITY,
        ),
        (
            NoiseModel::from_normalized(1.0, -1.0),
            "read_noise_electrons",
            -1.0,
        ),
        (
            NoiseModel::from_normalized(1.0, f32::INFINITY),
            "read_noise_electrons",
            f64::INFINITY,
        ),
    ];
    for (model, field, value) in cases {
        let invalid = model.validate().unwrap_err();
        assert_eq!((invalid.field, invalid.value), (field, value));
    }
}

#[test]
fn centroid_method_validate() {
    assert_eq!(CentroidMethod::WeightedMoments.validate(), Ok(()));
    assert_eq!(CentroidMethod::GaussianFit.validate(), Ok(()));
    assert_eq!(CentroidMethod::MoffatFit { beta: 2.5 }.validate(), Ok(()));
}

#[test]
fn centroid_method_invalid_beta_returns_exact_error() {
    for beta in [0.0, 15.0, f32::INFINITY] {
        let invalid = CentroidMethod::MoffatFit { beta }.validate().unwrap_err();
        assert_eq!(
            (invalid.field, invalid.value),
            ("Moffat beta", f64::from(beta))
        );
    }
}

#[test]
fn config_default() {
    let config = Config::default();
    assert!(config.measurement.noise_model.is_none());
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn config_presets() {
    assert_eq!(Config::wide_field().validate(), Ok(()));
    assert_eq!(Config::high_resolution().validate(), Ok(()));
    assert_eq!(Config::crowded_field().validate(), Ok(()));
    assert_eq!(Config::precise_ground().validate(), Ok(()));
}

#[test]
fn fwhm_modes_validate_and_seed_from_their_value() {
    // No filter at all, a fixed width, and an estimate that falls back to its own value: all
    // valid, and the seed is the one width each carries.
    for (mode, seed) in [
        (None, None),
        (Some(FwhmMode::Fixed(3.5)), Some(3.5)),
        (Some(FwhmMode::Auto { fallback: 4.5 }), Some(4.5)),
    ] {
        let config = configured(|config| config.fwhm.mode = mode);
        assert_eq!(config.validate(), Ok(()), "{mode:?}");
        assert_eq!(mode.map(FwhmMode::seed), seed);
    }
}

#[test]
fn inclusive_bounds_accept_their_edges() {
    // The rejection table below covers each bound's far side; these are the values that must
    // still pass. Every bound is expressed as the accepted half, so an inverted comparison
    // shows up here rather than as a config that silently stops working.
    type Edge = (&'static str, fn(&mut Config));
    let edges: [Edge; 11] = [
        ("read_noise 0", |c| {
            c.measurement.noise_model = Some(NoiseModel::from_normalized(1.0, 0.0));
        }),
        ("psf_axis_ratio 1", |c| c.detection.psf_axis_ratio = 1.0),
        ("deblend min_prominence 0", |c| {
            c.detection.deblend = Deblend::LocalMaxima {
                min_prominence: 0.0,
            };
        }),
        ("deblend min_prominence 1", |c| {
            c.detection.deblend = Deblend::LocalMaxima {
                min_prominence: 1.0,
            };
        }),
        ("deblend min_contrast 1", |c| {
            c.detection.deblend = Deblend::MultiThreshold {
                n_thresholds: 2,
                min_contrast: 1.0,
            };
        }),
        ("deblend min_contrast 0", |c| {
            c.detection.deblend = Deblend::MultiThreshold {
                n_thresholds: MAX_DEBLEND_N_THRESHOLDS,
                min_contrast: 0.0,
            };
        }),
        ("background refinement mask_dilation 50", |c| {
            c.background.refinement = BackgroundRefinement::Iterative {
                iterations: 10,
                mask_dilation: 50,
            };
        }),
        ("estimation_sigma_factor 1", |c| {
            c.fwhm.estimation_sigma_factor = 1.0;
        }),
        ("max_sharpness 1", |c| c.filter.max_sharpness = 1.0),
        ("max_fwhm_deviation None", |c| {
            c.filter.max_fwhm_deviation = None;
        }),
        ("duplicate_min_separation 0", |c| {
            c.filter.duplicate_min_separation = 0.0;
        }),
    ];

    for (label, apply) in edges {
        let config = configured(apply);
        assert_eq!(config.validate(), Ok(()), "{label} sits inside its bound");
    }
}

#[test]
fn config_invalid_parameters_return_exact_errors() {
    let cases = [
        (
            configured(|config| config.background.tile_size = 10),
            "tile_size",
            10.0,
        ),
        (
            configured(|config| config.background.sigma_clip_iterations = 11),
            "sigma_clip_iterations",
            11.0,
        ),
        (
            configured(|config| {
                config.background.refinement = BackgroundRefinement::Iterative {
                    iterations: 0,
                    mask_dilation: 3,
                };
            }),
            "background refinement iterations",
            0.0,
        ),
        (
            configured(|config| {
                config.background.refinement = BackgroundRefinement::Iterative {
                    iterations: 11,
                    mask_dilation: 3,
                };
            }),
            "background refinement iterations",
            11.0,
        ),
        (
            configured(|config| {
                config.background.refinement = BackgroundRefinement::Iterative {
                    iterations: 1,
                    mask_dilation: 51,
                };
            }),
            "background refinement mask_dilation",
            51.0,
        ),
        (
            configured(|config| config.detection.sigma_threshold = 0.0),
            "sigma_threshold",
            0.0,
        ),
        (
            configured(|config| config.fwhm.mode = Some(FwhmMode::Fixed(-1.0))),
            "fwhm Fixed",
            -1.0,
        ),
        (
            configured(|config| config.fwhm.mode = Some(FwhmMode::Fixed(0.0))),
            "fwhm Fixed",
            0.0,
        ),
        (
            configured(|config| config.fwhm.mode = Some(FwhmMode::Auto { fallback: 0.0 })),
            "fwhm Auto fallback",
            0.0,
        ),
        (
            configured(|config| config.detection.psf_axis_ratio = 0.0),
            "psf_axis_ratio",
            0.0,
        ),
        (
            configured(|config| config.detection.psf_angle = f32::INFINITY),
            "psf_angle",
            f64::INFINITY,
        ),
        (
            configured(|config| config.fwhm.min_stars = 4),
            "fwhm min_stars",
            4.0,
        ),
        (
            configured(|config| config.fwhm.estimation_sigma_factor = 0.5),
            "fwhm estimation_sigma_factor",
            0.5,
        ),
        (
            configured(|config| config.detection.deblend_min_separation = 0),
            "deblend_min_separation",
            0.0,
        ),
        (
            configured(|config| {
                config.detection.deblend = Deblend::LocalMaxima {
                    min_prominence: 1.5,
                };
            }),
            "deblend min_prominence",
            1.5,
        ),
        (
            configured(|config| config.detection.deblend = multi_threshold(1)),
            "deblend n_thresholds",
            1.0,
        ),
        (
            configured(|config| {
                config.detection.deblend = multi_threshold(MAX_DEBLEND_N_THRESHOLDS + 1);
            }),
            "deblend n_thresholds",
            (MAX_DEBLEND_N_THRESHOLDS + 1) as f64,
        ),
        (
            configured(|config| {
                config.detection.deblend = Deblend::MultiThreshold {
                    n_thresholds: 32,
                    min_contrast: -0.1,
                };
            }),
            "deblend min_contrast",
            // -0.1 has no exact f64 twin: compare against the f32 the field actually holds.
            f64::from(-0.1f32),
        ),
        (
            configured(|config| config.detection.min_area = 0),
            "min_area",
            0.0,
        ),
        (
            configured(|config| {
                config.detection.min_area = 100;
                config.detection.max_area = 50;
            }),
            "max_area",
            50.0,
        ),
        (
            configured(|config| {
                config.measurement.centroid_method = CentroidMethod::MoffatFit { beta: 0.0 };
            }),
            "Moffat beta",
            0.0,
        ),
        (
            configured(|config| config.filter.min_snr = 0.0),
            "min_snr",
            0.0,
        ),
        (
            configured(|config| config.filter.max_eccentricity = 1.5),
            "max_eccentricity",
            1.5,
        ),
        (
            configured(|config| config.filter.max_sharpness = 0.0),
            "max_sharpness",
            0.0,
        ),
        (
            configured(|config| config.filter.max_roundness = 0.0),
            "max_roundness",
            0.0,
        ),
        (
            configured(|config| config.filter.max_fwhm_deviation = Some(-1.0)),
            "max_fwhm_deviation",
            -1.0,
        ),
        (
            configured(|config| config.filter.max_fwhm_deviation = Some(0.0)),
            "max_fwhm_deviation",
            0.0,
        ),
        (
            configured(|config| config.filter.duplicate_min_separation = -1.0),
            "duplicate_min_separation",
            -1.0,
        ),
        (
            configured(|config| {
                config.measurement.noise_model = Some(NoiseModel::from_normalized(0.0, 1.0));
            }),
            "electrons_per_normalized_unit",
            0.0,
        ),
        (
            configured(|config| {
                config.measurement.noise_model = Some(NoiseModel::from_normalized(1.0, -1.0));
            }),
            "read_noise_electrons",
            -1.0,
        ),
    ];

    for (config, field, value) in cases {
        let invalid = config.validate().unwrap_err();
        assert_eq!((invalid.field, invalid.value), (field, value));
    }
}

#[test]
fn a_bound_that_is_another_config_value_is_reported_with_it() {
    let invalid = configured(|config| {
        config.detection.min_area = 100;
        config.detection.max_area = 50;
    })
    .validate()
    .unwrap_err();
    assert_eq!(
        invalid.to_string(),
        "max_area must be at least min_area (100), got 50"
    );

    let invalid = configured(|config| config.detection.deblend = multi_threshold(1))
        .validate()
        .unwrap_err();
    assert_eq!(
        invalid.to_string(),
        format!(
            "deblend n_thresholds must be between 2 and the deblend level cap ({MAX_DEBLEND_N_THRESHOLDS}), got 1"
        )
    );
}

/// Every float field rejects every non-finite value. NaN is the one a comparison-phrased check
/// lets through — every comparison with it is false — so it is the case that matters most; it is
/// reported as NaN, which `assert_eq!` on the value cannot see.
#[test]
fn config_rejects_non_finite_float_parameters() {
    type Field = (&'static str, fn(&mut Config, f32));
    let fields: [Field; 17] = [
        ("sigma_threshold", |c, v| c.detection.sigma_threshold = v),
        ("psf_axis_ratio", |c, v| c.detection.psf_axis_ratio = v),
        ("psf_angle", |c, v| c.detection.psf_angle = v),
        ("fwhm Fixed", |c, v| c.fwhm.mode = Some(FwhmMode::Fixed(v))),
        ("fwhm Auto fallback", |c, v| {
            c.fwhm.mode = Some(FwhmMode::Auto { fallback: v });
        }),
        ("fwhm estimation_sigma_factor", |c, v| {
            c.fwhm.estimation_sigma_factor = v;
        }),
        ("deblend min_prominence", |c, v| {
            c.detection.deblend = Deblend::LocalMaxima { min_prominence: v };
        }),
        ("deblend min_contrast", |c, v| {
            c.detection.deblend = Deblend::MultiThreshold {
                n_thresholds: 32,
                min_contrast: v,
            };
        }),
        ("min_snr", |c, v| c.filter.min_snr = v),
        ("max_eccentricity", |c, v| c.filter.max_eccentricity = v),
        ("max_sharpness", |c, v| c.filter.max_sharpness = v),
        ("max_roundness", |c, v| c.filter.max_roundness = v),
        ("max_fwhm_deviation", |c, v| {
            c.filter.max_fwhm_deviation = Some(v);
        }),
        ("Moffat beta", |c, v| {
            c.measurement.centroid_method = CentroidMethod::MoffatFit { beta: v };
        }),
        ("electrons_per_normalized_unit", |c, v| {
            c.measurement.noise_model = Some(NoiseModel::from_normalized(v, 5.0));
        }),
        ("read_noise_electrons", |c, v| {
            c.measurement.noise_model = Some(NoiseModel::from_normalized(1.0, v));
        }),
        ("duplicate_min_separation", |c, v| {
            c.filter.duplicate_min_separation = v;
        }),
    ];
    for (field, set) in fields {
        for value in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            let invalid = configured(|config| set(config, value))
                .validate()
                .unwrap_err();
            assert_eq!(invalid.field, field, "{field} = {value}");
            if value.is_nan() {
                assert!(
                    invalid.value.is_nan(),
                    "{field} = NaN reported {}",
                    invalid.value
                );
            } else {
                assert_eq!(invalid.value, f64::from(value), "{field} = {value}");
            }
        }
    }
}
