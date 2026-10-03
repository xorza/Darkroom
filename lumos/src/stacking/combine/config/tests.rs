use crate::stacking::combine::config::*;
use crate::stacking::combine::rejection::gesd_config::GesdConfig;
use crate::stacking::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::stacking::combine::rejection::percentile_clip_config::PercentileClipConfig;
use crate::stacking::combine::rejection::sigma_clip_config::SigmaClipConfig;
use crate::stacking::combine::rejection::winsorized_clip_config::WinsorizedClipConfig;

#[test]
fn small_n_resolve_downgrades_below_min_frames() {
    let sigma = CombineMethod::Mean(Rejection::sigma_clip(2.5));
    let floor5 = SmallN::median_below(5);
    // Below the floor → fallback (median); at/above → the configured method.
    assert_eq!(floor5.resolve(sigma, 4), CombineMethod::Median);
    assert_eq!(floor5.resolve(sigma, 5), sigma);
    assert_eq!(floor5.resolve(sigma, 50), sigma);
    // `none()` never downgrades, even at N=2.
    let win = CombineMethod::Mean(Rejection::winsorized(3.0));
    assert_eq!(SmallN::none().resolve(win, 2), win);
    // A method that already equals the fallback is returned unchanged (no spurious downgrade).
    assert_eq!(
        floor5.resolve(CombineMethod::Median, 1),
        CombineMethod::Median
    );
    // The flat preset's stricter floor of 8 is honoured.
    assert_eq!(
        StackConfig::flat().small_n.resolve(sigma, 7),
        CombineMethod::Median
    );
    assert_eq!(StackConfig::flat().small_n.resolve(sigma, 8), sigma);
}

/// Manual weights follow their frames past a drop: inputs 1 and 3 gone from five leave the
/// weights of 0, 2 and 4. Other weightings carry through untouched.
#[test]
fn for_survivors_keeps_the_weights_of_the_frames_left() {
    let config = StackConfig {
        weighting: Weighting::Manual(vec![1.0, 2.0, 3.0, 4.0, 5.0]),
        ..Default::default()
    };
    let Weighting::Manual(weights) = config.for_survivors(&[1, 3]).weighting else {
        panic!("manual stays manual")
    };
    assert_eq!(weights, [1.0, 3.0, 5.0]);
    let Weighting::Manual(all) = config.for_survivors(&[]).weighting else {
        panic!("manual stays manual")
    };
    assert_eq!(all, [1.0, 2.0, 3.0, 4.0, 5.0]);
    assert!(matches!(
        StackConfig::default().for_survivors(&[0]).weighting,
        Weighting::Equal
    ));
}

/// Every preset's method, weighting, normalization and small-stack fallback.
#[test]
fn presets_configure_as_documented() {
    let mean = CombineMethod::Mean;
    let floor = SmallN::median_below(MIN_FRAMES_FOR_REJECTION);
    for (name, config, method, weighting, normalization, small_n) in [
        (
            "default",
            StackConfig::default(),
            mean(Rejection::default()),
            Weighting::Equal,
            Normalization::None,
            floor,
        ),
        (
            "sigma clipped",
            StackConfig::sigma_clipped(2.0),
            mean(Rejection::sigma_clip(2.0)),
            Weighting::Equal,
            Normalization::None,
            floor,
        ),
        (
            "median",
            StackConfig::median(),
            CombineMethod::Median,
            Weighting::Equal,
            Normalization::None,
            SmallN::none(),
        ),
        (
            "mean",
            StackConfig::mean(),
            mean(Rejection::None),
            Weighting::Equal,
            Normalization::None,
            SmallN::none(),
        ),
        (
            "weighted",
            StackConfig::weighted(vec![1.0, 2.0, 3.0]),
            mean(Rejection::default()),
            Weighting::Manual(vec![1.0, 2.0, 3.0]),
            Normalization::None,
            floor,
        ),
        (
            "winsorized",
            StackConfig::winsorized(2.0),
            mean(Rejection::winsorized(2.0)),
            Weighting::Equal,
            Normalization::None,
            SmallN::none(),
        ),
        (
            "linear fit",
            StackConfig::linear_fit(2.0),
            mean(Rejection::linear_fit(2.0)),
            Weighting::Equal,
            Normalization::None,
            floor,
        ),
        (
            "percentile",
            StackConfig::percentile(15.0),
            mean(Rejection::percentile(15.0)),
            Weighting::Equal,
            Normalization::None,
            SmallN::none(),
        ),
        (
            "gesd",
            StackConfig::gesd(),
            mean(Rejection::gesd()),
            Weighting::Equal,
            Normalization::None,
            SmallN::median_below(MIN_FRAMES_FOR_GESD),
        ),
        (
            "bias",
            StackConfig::bias(),
            mean(Rejection::winsorized(3.0)),
            Weighting::Equal,
            Normalization::None,
            SmallN::none(),
        ),
        (
            "dark",
            StackConfig::dark(),
            mean(Rejection::winsorized(3.0)),
            Weighting::Equal,
            Normalization::None,
            SmallN::none(),
        ),
        (
            "flat",
            StackConfig::flat(),
            mean(Rejection::sigma_clip(3.0)),
            Weighting::Equal,
            Normalization::Multiplicative,
            SmallN::median_below(8),
        ),
        (
            "light",
            StackConfig::light(),
            mean(Rejection::sigma_clip(2.5)),
            Weighting::Noise,
            Normalization::Global,
            floor,
        ),
    ] {
        assert_eq!(config.method, method, "{name}");
        assert_eq!(config.weighting, weighting, "{name}");
        assert_eq!(config.normalization, normalization, "{name}");
        assert_eq!(config.small_n, small_n, "{name}");
    }
    assert_eq!((MIN_FRAMES_FOR_REJECTION, MIN_FRAMES_FOR_GESD), (5, 15));
}

#[test]
fn validate_valid_config() {
    let config = StackConfig::sigma_clipped(2.5);
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn validate_invalid_config_returns_exact_errors() {
    // Each case: the config, and the field its rejection must name with the value it carries.
    let range_checks = [
        (StackConfig::sigma_clipped(-1.0), "sigma_low", -1.0),
        (
            StackConfig {
                method: CombineMethod::Mean(Rejection::sigma_clip_asymmetric(2.0, f32::INFINITY)),
                ..Default::default()
            },
            "sigma_high",
            f64::INFINITY,
        ),
        (
            StackConfig {
                method: CombineMethod::Mean(Rejection::SigmaClip(SigmaClipConfig::new(2.0, 0))),
                ..Default::default()
            },
            "max_iterations",
            0.0,
        ),
        (
            StackConfig {
                method: CombineMethod::Mean(Rejection::Winsorized(WinsorizedClipConfig::new(0.0))),
                ..Default::default()
            },
            "sigma_low",
            0.0,
        ),
        (
            StackConfig {
                method: CombineMethod::Mean(Rejection::LinearFit(LinearFitClipConfig::new(
                    2.0, 0.0, 3,
                ))),
                ..Default::default()
            },
            "sigma_high",
            0.0,
        ),
        (StackConfig::percentile(60.0), "low_percentile", 60.0),
        (
            StackConfig {
                method: CombineMethod::Mean(Rejection::Percentile(PercentileClipConfig::new(
                    10.0, 60.0,
                ))),
                ..Default::default()
            },
            "high_percentile",
            60.0,
        ),
        (
            StackConfig {
                method: CombineMethod::Mean(Rejection::Percentile(PercentileClipConfig::new(
                    50.0, 50.0,
                ))),
                ..Default::default()
            },
            "low_percentile + high_percentile",
            100.0,
        ),
        (
            StackConfig {
                method: CombineMethod::Mean(Rejection::Gesd(GesdConfig::new(1.0, None))),
                ..Default::default()
            },
            "GESD alpha",
            1.0,
        ),
    ];
    for (config, field, value) in range_checks {
        let StackConfigError::Field(invalid) = config.validate().unwrap_err() else {
            panic!("{field} should be reported as an out-of-range field")
        };
        assert_eq!((invalid.field, invalid.value), (field, value));
    }

    // The constraints that aren't a range check on one field keep their own variant.
    let structural = [
        (
            StackConfig::weighted(vec![1.0, -0.5]),
            StackConfigError::InvalidManualWeight {
                index: 1,
                value: -0.5,
            },
        ),
        (
            StackConfig::weighted(vec![0.0, 0.0]),
            StackConfigError::InvalidManualWeightSum,
        ),
        (
            StackConfig {
                small_n: SmallN {
                    min_frames: 5,
                    fallback: CombineMethod::Mean(Rejection::sigma_clip(2.0)),
                },
                ..Default::default()
            },
            StackConfigError::RejectingSmallNFallback,
        ),
    ];
    for (config, expected) in structural {
        assert_eq!(config.validate(), Err(expected));
    }
}
