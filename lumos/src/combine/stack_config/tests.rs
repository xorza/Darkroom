use crate::combine::rejection::gesd_config::GesdConfig;
use crate::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::combine::rejection::sigma_clip_config::SigmaClipConfig;
use crate::combine::rejection::trim_config::TrimConfig;
use crate::combine::rejection::winsorized_clip_config::WinsorizedClipConfig;
use crate::combine::stack_config::combine::{MIN_FRAMES_FOR_GESD, MIN_FRAMES_FOR_REJECTION};
use crate::combine::stack_config::*;
use crate::pipeline::align_stack_config::AlignStackConfig;

/// A pixel of fewer samples than the floor falls back; at and above it, the configured method
/// runs. `none()` never falls back, even at 2, and neither a method that already equals the
/// fallback nor one that rejects nothing — the median under a mean fallback — has anything to fall
/// back from. The flat preset's stricter floor of 8 is honoured.
#[test]
fn small_n_falls_back_below_min_frames() {
    let sigma = CombineMethod::Mean(Rejection::sigma_clip(2.5));
    let floor5 = SmallN::median_below(5);
    assert!(floor5.falls_back(sigma, 4));
    assert!(!floor5.falls_back(sigma, 5));
    assert!(!floor5.falls_back(sigma, 50));
    let win = CombineMethod::Mean(Rejection::winsorized(3.0));
    assert!(!SmallN::none().falls_back(win, 2));
    assert!(!floor5.falls_back(CombineMethod::Median, 1));
    assert!(!floor5.falls_back(CombineMethod::Mean(Rejection::None), 1));
    let mean_below5 = SmallN {
        min_frames: 5,
        fallback: CombineMethod::Mean(Rejection::None),
    };
    assert!(!mean_below5.falls_back(CombineMethod::Median, 1));
    assert!(mean_below5.falls_back(sigma, 4));
    let flat = StackConfig::flat().combine.small_n;
    assert!(flat.falls_back(sigma, 7));
    assert!(!flat.falls_back(sigma, 8));
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
            Combine::sigma_clip(2.0),
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
            Combine::sigma_clip(2.5),
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
        combine: Combine::sigma_clip(2.5),
        weighting: Weighting::Equal,
        normalization: Normalization::None,
        ..StackConfig::light()
    };
    assert_eq!(config.validate(3), Ok(()));
}

#[test]
fn validate_invalid_config_returns_exact_errors() {
    // Each case: the config, and the field its rejection must name with the value it carries.
    let range_checks = [
        (
            StackConfig {
                combine: Combine::sigma_clip(-1.0),
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "sigma.low",
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
                    method: CombineMethod::Mean(Rejection::SigmaClip(SigmaClipConfig::new(
                        SigmaBounds::asymmetric(2.0, f32::INFINITY),
                        3,
                    ))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "sigma.high",
            f64::INFINITY,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::SigmaClip(SigmaClipConfig::new(
                        SigmaBounds::symmetric(2.0),
                        0,
                    ))),
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
                        SigmaBounds::symmetric(0.0),
                    ))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "sigma.low",
            0.0,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::LinearFit(LinearFitClipConfig::new(
                        SigmaBounds::asymmetric(2.0, 0.0),
                        4,
                    ))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "sigma.high",
            0.0,
        ),
        // One pass is the median clip alone: the line is never fitted.
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::LinearFit(LinearFitClipConfig::new(
                        SigmaBounds::symmetric(3.0),
                        1,
                    ))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "max_iterations",
            1.0,
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
            "alpha",
            1.0,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::Gesd(GesdConfig::new(0.0, None))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "alpha",
            0.0,
        ),
        (
            StackConfig {
                combine: Combine {
                    method: CombineMethod::Mean(Rejection::Gesd(GesdConfig::new(0.05, Some(0)))),
                    small_n: SmallN::median_below(5),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "max_outliers",
            0.0,
        ),
    ];
    for (config, field, value) in range_checks {
        let StackConfigError::Field(invalid) = config.validate(2).unwrap_err() else {
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
                idx: 1,
                value: -0.5,
            },
        ),
        (
            StackConfig {
                weighting: Weighting::Manual(vec![1.0, 0.0]),
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            StackConfigError::InvalidManualWeight { idx: 1, value: 0.0 },
        ),
        (
            StackConfig {
                weighting: Weighting::Manual(vec![]),
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
                    ..Combine::sigma_clip(2.5)
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            StackConfigError::RejectingSmallNFallback,
        ),
    ];
    for (config, expected) in structural {
        assert_eq!(config.validate(2), Err(expected));
    }

    // One manual weight per frame: two weights validate for two frames and for no other count.
    let weighted = StackConfig {
        weighting: Weighting::Manual(vec![1.0, 2.0]),
        ..StackConfig::light()
    };
    assert_eq!(weighted.validate(2), Ok(()));
    assert_eq!(
        weighted.validate(3),
        Err(StackConfigError::ManualWeightCountMismatch {
            expected: 3,
            actual: 2
        })
    );
}
