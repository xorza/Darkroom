use crate::combine::config::*;
use crate::combine::rejection::gesd_config::GesdConfig;
use crate::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::combine::rejection::sigma_clip_config::SigmaClipConfig;
use crate::combine::rejection::trim_config::TrimConfig;
use crate::combine::rejection::winsorized_clip_config::WinsorizedClipConfig;
use crate::pipeline::config::AlignStackConfig;

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
        StackConfig::flat().combine.small_n.resolve(sigma, 7),
        CombineMethod::Median
    );
    assert_eq!(StackConfig::flat().combine.small_n.resolve(sigma, 8), sigma);
}

/// Manual weights follow their frames past a drop: inputs 1 and 3 gone from five leave the
/// weights of 0, 2 and 4. Other weightings carry through untouched.
#[test]
fn for_survivors_keeps_the_weights_of_the_frames_left() {
    let config = StackConfig {
        weighting: Weighting::Manual(vec![1.0, 2.0, 3.0, 4.0, 5.0]),
        normalization: Normalization::None,
        ..StackConfig::light()
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
        StackConfig {
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            ..StackConfig::light()
        }
        .for_survivors(&[0])
        .weighting,
        Weighting::Equal
    ));
}

/// Every method preset's method and small-stack fallback, and nothing else: the policy around it
/// is the role's.
#[test]
fn method_presets_set_the_method_alone() {
    let mean = CombineMethod::Mean;
    let floor = SmallN::median_below(MIN_FRAMES_FOR_REJECTION);
    for (name, combine, method, small_n) in [
        (
            "sigma clipped",
            Combine::sigma_clipped(2.0),
            mean(Rejection::sigma_clip(2.0)),
            floor,
        ),
        (
            "median",
            Combine::median(),
            CombineMethod::Median,
            SmallN::none(),
        ),
        (
            "mean",
            Combine::mean(),
            mean(Rejection::None),
            SmallN::none(),
        ),
        (
            "winsorized",
            Combine::winsorized(2.0),
            mean(Rejection::winsorized(2.0)),
            SmallN::none(),
        ),
        (
            "linear fit",
            Combine::linear_fit(2.0),
            mean(Rejection::linear_fit(2.0)),
            floor,
        ),
        (
            "trim",
            Combine::trim(15.0),
            mean(Rejection::trim(15.0)),
            SmallN::none(),
        ),
        (
            "gesd",
            Combine::gesd(),
            mean(Rejection::gesd()),
            SmallN::median_below(MIN_FRAMES_FOR_GESD),
        ),
    ] {
        assert_eq!(combine, Combine { method, small_n }, "{name}");
    }
    assert_eq!((MIN_FRAMES_FOR_REJECTION, MIN_FRAMES_FOR_GESD), (5, 15));
}

/// Every role preset's combine, weighting and normalization: lights are normalized and weighted
/// by their noise, masters neither weighted nor normalized but the flats, which are scaled to one
/// another.
#[test]
fn role_presets_set_the_policy_of_their_frames() {
    for (name, config, combine, weighting, normalization) in [
        (
            "light",
            StackConfig::light(),
            Combine::sigma_clipped(2.5),
            Weighting::Noise,
            Normalization::Global,
        ),
        (
            "bias or dark",
            StackConfig::bias_or_dark(),
            Combine::winsorized(3.0),
            Weighting::Equal,
            Normalization::None,
        ),
        (
            "flat",
            StackConfig::flat(),
            Combine {
                method: CombineMethod::Mean(Rejection::sigma_clip(3.0)),
                small_n: SmallN::median_below(8),
            },
            Weighting::Equal,
            Normalization::Multiplicative,
        ),
    ] {
        assert_eq!(config.combine, combine, "{name}");
        assert_eq!(config.weighting, weighting, "{name}");
        assert_eq!(config.normalization, normalization, "{name}");
        assert_eq!(config.quality, QualityPlanes::STANDARD, "{name}");
        assert_eq!(config.min_survivors, DEFAULT_MIN_SURVIVORS, "{name}");
    }
    // A registered stack is of lights unless its caller says otherwise.
    assert_eq!(AlignStackConfig::default().stack, StackConfig::light());
}

#[test]
fn validate_valid_config() {
    let config = StackConfig {
        combine: Combine::sigma_clipped(2.5),
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn validate_invalid_config_returns_exact_errors() {
    // Each case: the config, and the field its rejection must name with the value it carries.
    let range_checks = [
        (
            StackConfig {
                combine: Combine::sigma_clipped(-1.0),
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "sigma_low",
            -1.0,
        ),
        (
            StackConfig {
                min_survivors: 0,
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "min_survivors",
            0.0,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::sigma_clip_asymmetric(
                        2.0,
                        f32::INFINITY,
                    )),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "sigma_high",
            f64::INFINITY,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::SigmaClip(SigmaClipConfig::new(2.0, 0))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "max_iterations",
            0.0,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::Winsorized(WinsorizedClipConfig::new(
                        0.0,
                    ))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "sigma_low",
            0.0,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::LinearFit(LinearFitClipConfig::new(
                        2.0, 0.0, 3,
                    ))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "sigma_high",
            0.0,
        ),
        (
            StackConfig {
                combine: Combine::trim(60.0),
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "low_percent",
            60.0,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::Trim(TrimConfig::new(10.0, 60.0))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "high_percent",
            60.0,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::Trim(TrimConfig::new(50.0, 50.0))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "low_percent + high_percent",
            100.0,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::Gesd(GesdConfig::new(1.0, None))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
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
            StackConfig {
                weighting: Weighting::Manual(vec![1.0, -0.5]),
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            StackConfigError::InvalidManualWeight {
                index: 1,
                value: -0.5,
            },
        ),
        (
            StackConfig {
                weighting: Weighting::Manual(vec![0.0, 0.0]),
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            StackConfigError::InvalidManualWeightSum,
        ),
        (
            StackConfig {
                combine: Combine {
                    small_n: SmallN {
                        min_frames: 5,
                        fallback: CombineMethod::Mean(Rejection::sigma_clip(2.0)),
                    },
                    ..Combine::sigma_clipped(2.5)
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            StackConfigError::RejectingSmallNFallback,
        ),
    ];
    for (config, expected) in structural {
        assert_eq!(config.validate(), Err(expected));
    }
}
