use crate::stacking::star_detection::config::background_config::BackgroundRefinement;
use crate::stacking::star_detection::config::detection_config::{
    Connectivity, MAX_DEBLEND_N_THRESHOLDS,
};
use crate::stacking::star_detection::config::measurement_config::{
    CentroidMethod, LocalBackgroundMethod, NoiseModel,
};
use crate::stacking::star_detection::config::*;

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
        assert_eq!((invalid.field, invalid.value), ("Moffat beta", beta as f64));
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
fn config_custom() {
    let config = configured(|config| {
        config.fwhm.expected = 5.0;
        config.filter.min_snr = 15.0;
        config.detection.edge_margin = 20;
        config.measurement.noise_model = Some(NoiseModel::from_normalized(24_000.0, 5.0));
    });

    assert!((config.fwhm.expected - 5.0).abs() < 1e-6);
    assert!((config.filter.min_snr - 15.0).abs() < 1e-6);
    assert_eq!(config.detection.edge_margin, 20);
    assert!(config.measurement.noise_model.is_some());
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn config_with_auto_fwhm() {
    let config = configured(|config| {
        config.fwhm.auto_estimate = true;
        config.fwhm.expected = 0.0;
    });
    assert!(config.fwhm.auto_estimate);
    assert!((config.fwhm.expected - 0.0).abs() < 1e-6);
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn config_validates_centroid() {
    let config = configured(|config| {
        config.measurement.centroid_method = CentroidMethod::MoffatFit { beta: 2.5 };
    });
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn inclusive_bounds_accept_their_edges() {
    // The rejection table below covers each bound's far side; these are the values that must
    // still pass. Every bound is expressed as the accepted half, so an inverted comparison
    // shows up here rather than as a config that silently stops working.
    type Edge = (&'static str, fn(&mut Config));
    let edges: [Edge; 10] = [
        ("read_noise 0", |c| {
            c.measurement.noise_model = Some(NoiseModel::from_normalized(1.0, 0.0));
        }),
        ("psf_axis_ratio 1", |c| c.detection.psf_axis_ratio = 1.0),
        ("deblend_min_prominence 0", |c| {
            c.detection.deblend_min_prominence = 0.0;
        }),
        ("deblend_min_prominence 1", |c| {
            c.detection.deblend_min_prominence = 1.0;
        }),
        ("deblend_min_contrast 1", |c| {
            c.detection.deblend_min_contrast = 1.0;
        }),
        ("expected fwhm 0", |c| c.fwhm.expected = 0.0),
        ("estimation_sigma_factor 1", |c| {
            c.fwhm.estimation_sigma_factor = 1.0;
        }),
        ("max_sharpness 1", |c| c.filter.max_sharpness = 1.0),
        ("max_fwhm_deviation 0", |c| {
            c.filter.max_fwhm_deviation = 0.0
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
                config.background.refinement = BackgroundRefinement::Iterative { iterations: 0 };
            }),
            "background refinement iterations",
            0.0,
        ),
        (
            configured(|config| {
                config.background.refinement = BackgroundRefinement::Iterative { iterations: 11 };
            }),
            "background refinement iterations",
            11.0,
        ),
        (
            configured(|config| config.background.mask_dilation = 51),
            "bg_mask_dilation",
            51.0,
        ),
        (
            configured(|config| config.detection.sigma_threshold = 0.0),
            "sigma_threshold",
            0.0,
        ),
        (
            configured(|config| config.fwhm.expected = -1.0),
            "expected_fwhm",
            -1.0,
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
            "min_stars_for_fwhm",
            4.0,
        ),
        (
            configured(|config| config.fwhm.estimation_sigma_factor = 0.5),
            "fwhm_estimation_sigma_factor",
            0.5,
        ),
        (
            configured(|config| config.detection.deblend_min_separation = 0),
            "deblend_min_separation",
            0.0,
        ),
        (
            configured(|config| config.detection.deblend_min_prominence = 1.5),
            "deblend_min_prominence",
            1.5,
        ),
        (
            configured(|config| config.detection.deblend_n_thresholds = 1),
            "deblend_n_thresholds",
            1.0,
        ),
        (
            configured(|config| {
                config.detection.deblend_n_thresholds = MAX_DEBLEND_N_THRESHOLDS + 1;
            }),
            "deblend_n_thresholds",
            (MAX_DEBLEND_N_THRESHOLDS + 1) as f64,
        ),
        (
            configured(|config| config.detection.deblend_min_contrast = -0.1),
            "deblend_min_contrast",
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
            configured(|config| config.filter.max_fwhm_deviation = -1.0),
            "max_fwhm_deviation",
            -1.0,
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

    let invalid = configured(|config| config.detection.deblend_n_thresholds = 1)
        .validate()
        .unwrap_err();
    assert_eq!(
        invalid.to_string(),
        format!(
            "deblend_n_thresholds must be 0, or between 2 and the deblend level cap ({MAX_DEBLEND_N_THRESHOLDS}), got 1"
        )
    );
}

#[test]
fn config_deblend_n_thresholds_at_max_accepted() {
    assert_eq!(
        configured(|config| {
            config.detection.deblend_n_thresholds = MAX_DEBLEND_N_THRESHOLDS;
        })
        .validate(),
        Ok(())
    );
}

#[test]
fn config_deblend_multi_threshold() {
    let config = configured(|config| config.detection.deblend_n_thresholds = 32);
    assert!(config.detection.is_multi_threshold());
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn config_wide_field_values() {
    let config = Config::wide_field();
    assert!((config.fwhm.expected - 6.0).abs() < 1e-6);
    assert!(config.fwhm.auto_estimate);
    assert_eq!(config.detection.min_area, 7);
    assert_eq!(config.detection.max_area, 1500);
    assert_eq!(config.detection.edge_margin, 20);
    assert!((config.filter.max_eccentricity - 0.7).abs() < 1e-6);
    assert_eq!(config.detection.connectivity, Connectivity::Eight);
}

#[test]
fn config_precise_ground_values() {
    let config = Config::precise_ground();
    assert!(matches!(
        config.measurement.centroid_method,
        CentroidMethod::MoffatFit { beta } if (beta - 2.5).abs() < 1e-6
    ));
    assert_eq!(
        config.measurement.local_background,
        LocalBackgroundMethod::LocalAnnulus
    );
    assert_eq!(config.detection.deblend_n_thresholds, 32);
    assert!((config.filter.min_snr - 15.0).abs() < 1e-6);
    assert_eq!(config.background.tile_size, 128);
    assert!((config.detection.sigma_threshold - 3.0).abs() < 1e-6);
    assert!(config.fwhm.auto_estimate);
    assert_eq!(config.fwhm.min_stars, 30);
}

#[test]
fn config_high_resolution_values() {
    let config = Config::high_resolution();
    assert!((config.fwhm.expected - 2.5).abs() < 1e-6);
    assert!(config.fwhm.auto_estimate);
    assert_eq!(config.detection.min_area, 3);
    assert_eq!(config.detection.max_area, 200);
    assert!((config.filter.min_snr - 15.0).abs() < 1e-6);
    assert!((config.filter.max_eccentricity - 0.5).abs() < 1e-6);
    assert!((config.filter.max_roundness - 0.3).abs() < 1e-6);
    assert!(matches!(
        config.measurement.centroid_method,
        CentroidMethod::GaussianFit
    ));
}

#[test]
fn config_crowded_field_values() {
    let config = Config::crowded_field();
    assert_eq!(config.detection.deblend_n_thresholds, 32);
    assert_eq!(config.detection.deblend_min_separation, 2);
    assert!((config.detection.deblend_min_prominence - 0.15).abs() < 1e-6);
    assert!((config.detection.deblend_min_contrast - 0.005).abs() < 1e-6);
    assert!(matches!(
        config.background.refinement,
        BackgroundRefinement::Iterative { iterations: 2 }
    ));
    assert!((config.filter.duplicate_min_separation - 3.0).abs() < 1e-6);
    assert!(config.fwhm.auto_estimate);
}

#[test]
fn config_rejects_non_finite_float_parameters() {
    let cases = [
        (
            configured(|config| config.detection.sigma_threshold = f32::INFINITY),
            "sigma_threshold",
            f64::INFINITY,
        ),
        (
            configured(|config| config.fwhm.expected = f32::INFINITY),
            "expected_fwhm",
            f64::INFINITY,
        ),
        (
            configured(|config| config.detection.psf_axis_ratio = f32::INFINITY),
            "psf_axis_ratio",
            f64::INFINITY,
        ),
        (
            configured(|config| config.fwhm.estimation_sigma_factor = f32::INFINITY),
            "fwhm_estimation_sigma_factor",
            f64::INFINITY,
        ),
        (
            configured(|config| {
                config.detection.deblend_min_prominence = f32::INFINITY;
            }),
            "deblend_min_prominence",
            f64::INFINITY,
        ),
        (
            configured(|config| config.detection.deblend_min_contrast = f32::INFINITY),
            "deblend_min_contrast",
            f64::INFINITY,
        ),
        (
            configured(|config| config.filter.min_snr = f32::INFINITY),
            "min_snr",
            f64::INFINITY,
        ),
        (
            configured(|config| config.filter.max_eccentricity = f32::INFINITY),
            "max_eccentricity",
            f64::INFINITY,
        ),
        (
            configured(|config| config.filter.max_sharpness = f32::INFINITY),
            "max_sharpness",
            f64::INFINITY,
        ),
        (
            configured(|config| config.filter.max_roundness = f32::INFINITY),
            "max_roundness",
            f64::INFINITY,
        ),
        (
            configured(|config| config.filter.max_fwhm_deviation = f32::INFINITY),
            "max_fwhm_deviation",
            f64::INFINITY,
        ),
        (
            configured(|config| config.filter.duplicate_min_separation = f32::INFINITY),
            "duplicate_min_separation",
            f64::INFINITY,
        ),
    ];

    for (config, field, value) in cases {
        let invalid = config.validate().unwrap_err();
        assert_eq!((invalid.field, invalid.value), (field, value));
    }
}

#[test]
fn background_refinement_iterations() {
    assert_eq!(BackgroundRefinement::None.iterations(), 0);
    assert_eq!(
        BackgroundRefinement::Iterative { iterations: 3 }.iterations(),
        3
    );
    assert_eq!(BackgroundRefinement::None.validate(), Ok(()));
    assert_eq!(
        BackgroundRefinement::Iterative { iterations: 3 }.validate(),
        Ok(())
    );
}

#[test]
fn background_refinement_invalid_iterations_return_exact_errors() {
    for iterations in [0, 11] {
        let invalid = BackgroundRefinement::Iterative { iterations }
            .validate()
            .unwrap_err();
        assert_eq!(
            invalid.to_string(),
            format!("background refinement iterations must be between 1 and 10, got {iterations}")
        );
    }
}

#[test]
fn test_is_multi_threshold() {
    // 0 = disabled → false
    let config = configured(|config| config.detection.deblend_n_thresholds = 0);
    assert!(!config.detection.is_multi_threshold());

    // >= 2 → true
    let config = configured(|config| config.detection.deblend_n_thresholds = 2);
    assert!(config.detection.is_multi_threshold());
}
