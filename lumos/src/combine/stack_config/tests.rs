use crate::combine::rejection::gesd_config::GesdConfig;
use crate::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::combine::rejection::sigma_bounds::SigmaBounds;
use crate::combine::rejection::sigma_clip_config::SigmaClipConfig;
use crate::combine::rejection::trim_config::{TrimBounds, TrimConfig};
use crate::combine::rejection::winsorized_clip_config::WinsorizedClipConfig;
use crate::combine::stack_config::*;
use crate::pipeline::align_stack_config::AlignStackConfig;

/// A pixel of fewer samples than the floor falls back; at and above it, the rejection runs. The
/// flat preset's stricter floor of 8 is honoured.
#[test]
fn small_n_falls_back_below_min_frames() {
    let floor5 = SmallN::median_below(5);
    assert!(floor5.falls_back(4));
    assert!(!floor5.falls_back(5));
    assert!(!floor5.falls_back(50));
    let Combine::RejectedMean {
        small_n: Some(flat),
        ..
    } = StackConfig::flat().combine
    else {
        panic!("the flat preset rejects with a floor")
    };
    assert!(flat.falls_back(7));
    assert!(!flat.falls_back(8));
}

/// Every method preset's rejection and small-stack fallback, and nothing else: the policy around
/// it is the role's. Sigma clip and linear fit fall back to the median below 5 frames, GESD below
/// 15; Winsorized and trim, which clip once about every sample or measure no spread, never.
#[test]
fn method_presets_set_the_method_alone() {
    let floor = Some(SmallN::median_below(5));
    for (name, combine, rejection, small_n) in [
        (
            "sigma clipped",
            Combine::sigma_clip(2.0),
            Rejection::sigma_clip(2.0),
            floor,
        ),
        (
            "winsorized",
            Combine::winsorized(2.0),
            Rejection::winsorized(2.0),
            None,
        ),
        (
            "linear fit",
            Combine::linear_fit(2.0),
            Rejection::linear_fit(2.0),
            floor,
        ),
        ("trim", Combine::trim(15.0), Rejection::trim(15.0), None),
        (
            "gesd",
            Combine::gesd(),
            Rejection::gesd(),
            Some(SmallN::median_below(15)),
        ),
    ] {
        assert_eq!(
            combine,
            Combine::RejectedMean { rejection, small_n },
            "{name}"
        );
        assert_eq!(rejection.small_n(), small_n, "{name}");
    }
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
            Combine::RejectedMean {
                rejection: Rejection::sigma_clip(3.0),
                small_n: Some(SmallN::median_below(8)),
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
    assert_eq!(config.validate(), Ok(()));
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
                combine: Combine::RejectedMean {
                    rejection: Rejection::SigmaClip(SigmaClipConfig {
                        sigma: SigmaBounds::asymmetric(2.0, f32::INFINITY),
                        max_iterations: 3,
                        ..SigmaClipConfig::DEFAULT
                    }),
                    small_n: Some(SmallN::median_below(5)),
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
                combine: Combine::RejectedMean {
                    rejection: Rejection::SigmaClip(SigmaClipConfig {
                        sigma: SigmaBounds::symmetric(2.0),
                        max_iterations: 0,
                        ..SigmaClipConfig::DEFAULT
                    }),
                    small_n: Some(SmallN::median_below(5)),
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
                combine: Combine::RejectedMean {
                    rejection: Rejection::Winsorized(WinsorizedClipConfig {
                        sigma: SigmaBounds::symmetric(0.0),
                    }),
                    small_n: Some(SmallN::median_below(5)),
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
                combine: Combine::RejectedMean {
                    rejection: Rejection::LinearFit(LinearFitClipConfig {
                        sigma: SigmaBounds::asymmetric(2.0, 0.0),
                        max_iterations: 4,
                    }),
                    small_n: Some(SmallN::median_below(5)),
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
                combine: Combine::RejectedMean {
                    rejection: Rejection::LinearFit(LinearFitClipConfig {
                        sigma: SigmaBounds::symmetric(3.0),
                        max_iterations: 1,
                    }),
                    small_n: Some(SmallN::median_below(5)),
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
            "percent.low",
            60.0,
        ),
        (
            StackConfig {
                combine: Combine::RejectedMean {
                    rejection: Rejection::Trim(TrimConfig {
                        percent: TrimBounds::asymmetric(10.0, 60.0),
                    }),
                    small_n: Some(SmallN::median_below(5)),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "percent.high",
            60.0,
        ),
        (
            StackConfig {
                combine: Combine::RejectedMean {
                    rejection: Rejection::Trim(TrimConfig {
                        percent: TrimBounds::symmetric(50.0),
                    }),
                    small_n: Some(SmallN::median_below(5)),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "percent.low + percent.high",
            100.0,
        ),
        (
            StackConfig {
                combine: Combine::RejectedMean {
                    rejection: Rejection::Gesd(GesdConfig {
                        alpha: 1.0,
                        outliers_percent: 30.0,
                    }),
                    small_n: Some(SmallN::median_below(5)),
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
                combine: Combine::RejectedMean {
                    rejection: Rejection::Gesd(GesdConfig {
                        alpha: 0.0,
                        outliers_percent: 30.0,
                    }),
                    small_n: Some(SmallN::median_below(5)),
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
                combine: Combine::RejectedMean {
                    rejection: Rejection::Gesd(GesdConfig {
                        outliers_percent: 0.0,
                        ..GesdConfig::DEFAULT
                    }),
                    small_n: Some(SmallN::median_below(5)),
                },
                weighting: Weighting::Equal,
                normalization: Normalization::None,
                ..StackConfig::light()
            },
            "outliers_percent",
            0.0,
        ),
        // A floor of 1 falls back at no pixel, a second spelling of none; one past 33 is past
        // the median's tabled variance.
        (
            StackConfig {
                combine: Combine::RejectedMean {
                    rejection: Rejection::sigma_clip(2.5),
                    small_n: Some(SmallN::median_below(1)),
                },
                ..StackConfig::light()
            },
            "small_n.min_frames",
            1.0,
        ),
        (
            StackConfig {
                combine: Combine::RejectedMean {
                    rejection: Rejection::sigma_clip(2.5),
                    small_n: Some(SmallN::median_below(34)),
                },
                ..StackConfig::light()
            },
            "small_n.min_frames",
            34.0,
        ),
    ];
    for (config, field, value) in range_checks {
        let invalid = config.validate().unwrap_err();
        assert_eq!((invalid.field, invalid.value), (field, value));
    }
}
