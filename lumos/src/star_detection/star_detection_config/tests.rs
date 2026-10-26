use crate::star_detection::star_detection_config::extraction_config::MAX_DEBLEND_THRESHOLD_COUNT;
use crate::star_detection::star_detection_config::*;

fn multi_threshold(threshold_count: usize) -> Deblend {
    Deblend::MultiThreshold {
        threshold_count,
        min_contrast: 0.005,
    }
}

fn configured(update: impl FnOnce(&mut StarDetectionConfig)) -> StarDetectionConfig {
    let mut config = StarDetectionConfig::default();
    update(&mut config);
    config
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
    let config = StarDetectionConfig::default();
    assert!(config.measurement.electrons_per_unit.is_none());
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn config_presets() {
    assert_eq!(StarDetectionConfig::wide_field().validate(), Ok(()));
    assert_eq!(StarDetectionConfig::high_resolution().validate(), Ok(()));
    assert_eq!(StarDetectionConfig::crowded_field().validate(), Ok(()));
    assert_eq!(StarDetectionConfig::precise_ground().validate(), Ok(()));
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
    type Edge = (&'static str, fn(&mut StarDetectionConfig));
    let edges: [Edge; 14] = [
        ("electrons_per_unit at the smallest positive f32", |c| {
            c.measurement.electrons_per_unit = Some(f32::MIN_POSITIVE);
        }),
        ("psf_axis_ratio 1", |c| c.fwhm.psf_axis_ratio = 1.0),
        ("fwhm Fixed 20, the widest star", |c| {
            c.fwhm.mode = Some(FwhmMode::Fixed(20.0));
        }),
        // On the pixel axes a thin ellipse filters as two 1D passes and needs no quadrature.
        ("psf_axis_ratio 1e-4 along the axes", |c| {
            c.fwhm.psf_axis_ratio = 1e-4;
            c.fwhm.psf_angle = 0.0;
        }),
        ("deblend min_relative_height 0", |c| {
            c.extraction.deblend = Deblend::LocalMaxima {
                min_relative_height: 0.0,
            };
        }),
        ("deblend min_relative_height 1", |c| {
            c.extraction.deblend = Deblend::LocalMaxima {
                min_relative_height: 1.0,
            };
        }),
        ("deblend min_contrast 1", |c| {
            c.extraction.deblend = Deblend::MultiThreshold {
                threshold_count: 2,
                min_contrast: 1.0,
            };
        }),
        ("deblend min_contrast 0", |c| {
            c.extraction.deblend = Deblend::MultiThreshold {
                threshold_count: MAX_DEBLEND_THRESHOLD_COUNT,
                min_contrast: 0.0,
            };
        }),
        ("background refinement mask_dilation 50", |c| {
            c.bg.refinement = BgRefinement::Iterative(IterativeRefinement {
                iterations: NonZeroUsize::new(10).unwrap(),
                mask_dilation: 50,
                mask_sigma: 2.0,
            });
        }),
        ("estimation_sigma_factor 1", |c| {
            c.fwhm.estimation_sigma_factor = 1.0;
        }),
        ("max_sharpness 1", |c| c.filter.max_sharpness = 1.0),
        ("max_roundness 2", |c| c.filter.max_roundness = 2.0),
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
            configured(|config| config.bg.tile_size = 10),
            "tile_size",
            10.0,
        ),
        (
            configured(|config| config.bg.sigma_clip_max_iterations = 11),
            "sigma_clip_max_iterations",
            11.0,
        ),
        (
            configured(|config| {
                config.bg.refinement = BgRefinement::Iterative(IterativeRefinement {
                    iterations: NonZeroUsize::new(11).unwrap(),
                    mask_dilation: 3,
                    mask_sigma: 2.0,
                });
            }),
            "background refinement iterations",
            11.0,
        ),
        (
            configured(|config| {
                config.bg.refinement = BgRefinement::Iterative(IterativeRefinement {
                    iterations: NonZeroUsize::MIN,
                    mask_dilation: 51,
                    mask_sigma: 2.0,
                });
            }),
            "background refinement mask_dilation",
            51.0,
        ),
        (
            configured(|config| {
                config.bg.refinement = BgRefinement::Iterative(IterativeRefinement {
                    iterations: NonZeroUsize::MIN,
                    mask_dilation: 3,
                    mask_sigma: 0.0,
                });
            }),
            "background refinement mask_sigma",
            0.0,
        ),
        (
            configured(|config| config.extraction.sigma_threshold = 0.0),
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
            configured(|config| config.fwhm.psf_axis_ratio = 0.0),
            "psf_axis_ratio",
            0.0,
        ),
        (
            configured(|config| config.fwhm.mode = Some(FwhmMode::Fixed(20.5))),
            "fwhm Fixed",
            20.5,
        ),
        (
            configured(|config| config.fwhm.mode = Some(FwhmMode::Auto { fallback: 25.0 })),
            "fwhm Auto fallback",
            25.0,
        ),
        // Off the axes, at FWHM 4, an axis ratio of 1e-4 leaves σ 1.7e-4 px across the pixel
        // axes, past what any pixel rule resolves: one fixed rule's every node would miss the
        // ridge, and the kernel would sum to 0.
        (
            configured(|config| {
                config.fwhm.psf_axis_ratio = 1e-4;
                config.fwhm.psf_angle = 0.3;
            }),
            "psf_axis_ratio",
            f64::from(1e-4f32),
        ),
        (
            configured(|config| config.fwhm.psf_angle = f32::INFINITY),
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
            configured(|config| config.extraction.deblend_min_separation = 0),
            "deblend_min_separation",
            0.0,
        ),
        (
            configured(|config| {
                config.extraction.deblend = Deblend::LocalMaxima {
                    min_relative_height: 1.5,
                };
            }),
            "deblend min_relative_height",
            1.5,
        ),
        (
            configured(|config| config.extraction.deblend = multi_threshold(1)),
            "deblend threshold_count",
            1.0,
        ),
        (
            configured(|config| {
                config.extraction.deblend = multi_threshold(MAX_DEBLEND_THRESHOLD_COUNT + 1);
            }),
            "deblend threshold_count",
            (MAX_DEBLEND_THRESHOLD_COUNT + 1) as f64,
        ),
        (
            configured(|config| {
                config.extraction.deblend = Deblend::MultiThreshold {
                    threshold_count: 32,
                    min_contrast: -0.1,
                };
            }),
            "deblend min_contrast",
            // -0.1 has no exact f64 twin: compare against the f32 the field actually holds.
            f64::from(-0.1f32),
        ),
        (
            configured(|config| config.extraction.min_area = 0),
            "min_area",
            0.0,
        ),
        (
            configured(|config| {
                config.extraction.min_area = 100;
                config.extraction.max_area = 50;
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
            configured(|config| config.measurement.electrons_per_unit = Some(0.0)),
            "electrons_per_unit",
            0.0,
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
        config.extraction.min_area = 100;
        config.extraction.max_area = 50;
    })
    .validate()
    .unwrap_err();
    assert_eq!(
        invalid.to_string(),
        "max_area must be at least min_area (100), got 50"
    );

    let invalid = configured(|config| config.extraction.deblend = multi_threshold(1))
        .validate()
        .unwrap_err();
    assert_eq!(
        invalid.to_string(),
        format!(
            "deblend threshold_count must be between 2 and the deblend level cap \
             ({MAX_DEBLEND_THRESHOLD_COUNT}), got 1"
        )
    );
}

/// Every float field rejects every non-finite value. NaN is the one a comparison-phrased check
/// lets through — every comparison with it is false — so it is the case that matters most; it is
/// reported as NaN, which `assert_eq!` on the value cannot see.
#[test]
fn config_rejects_non_finite_float_parameters() {
    type Field = (&'static str, fn(&mut StarDetectionConfig, f32));
    let fields: [Field; 16] = [
        ("sigma_threshold", |c, v| c.extraction.sigma_threshold = v),
        ("psf_axis_ratio", |c, v| c.fwhm.psf_axis_ratio = v),
        ("psf_angle", |c, v| c.fwhm.psf_angle = v),
        ("fwhm Fixed", |c, v| c.fwhm.mode = Some(FwhmMode::Fixed(v))),
        ("fwhm Auto fallback", |c, v| {
            c.fwhm.mode = Some(FwhmMode::Auto { fallback: v });
        }),
        ("fwhm estimation_sigma_factor", |c, v| {
            c.fwhm.estimation_sigma_factor = v;
        }),
        ("deblend min_relative_height", |c, v| {
            c.extraction.deblend = Deblend::LocalMaxima {
                min_relative_height: v,
            };
        }),
        ("deblend min_contrast", |c, v| {
            c.extraction.deblend = Deblend::MultiThreshold {
                threshold_count: 32,
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
        ("electrons_per_unit", |c, v| {
            c.measurement.electrons_per_unit = Some(v);
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
