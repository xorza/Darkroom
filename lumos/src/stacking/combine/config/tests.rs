use crate::stacking::combine::config::*;
use crate::stacking::combine::rejection::{
    GesdConfig, LinearFitClipConfig, PercentileClipConfig, SigmaClipConfig, WinsorizedClipConfig,
};

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

#[test]
fn test_default_config() {
    let config = StackConfig::default();
    assert!(matches!(
        config.method,
        CombineMethod::Mean(Rejection::SigmaClip(..))
    ));
    assert_eq!(config.weighting, Weighting::Equal);
    assert_eq!(config.normalization, Normalization::None);
}

#[test]
fn test_sigma_clipped_preset() {
    let config = StackConfig::sigma_clipped(2.0);
    assert!(matches!(
        config.method,
        CombineMethod::Mean(Rejection::SigmaClip(c))
            if (c.sigma_low - 2.0).abs() < f32::EPSILON && (c.sigma_high - 2.0).abs() < f32::EPSILON
    ));
}

#[test]
fn test_median_preset() {
    let config = StackConfig::median();
    assert_eq!(config.method, CombineMethod::Median);
}

#[test]
fn test_weighted_preset() {
    let config = StackConfig::weighted(vec![1.0, 2.0, 3.0]);
    assert!(matches!(config.method, CombineMethod::Mean(..)));
    assert!(matches!(config.weighting, Weighting::Manual(ref w) if w.len() == 3));
}

#[test]
fn test_struct_update_syntax() {
    let config = StackConfig {
        method: CombineMethod::Mean(Rejection::SigmaClip(SigmaClipConfig::new_asymmetric(
            2.0, 3.0, 5,
        ))),
        normalization: Normalization::Global,
        ..Default::default()
    };
    assert!(matches!(
        config.method,
        CombineMethod::Mean(Rejection::SigmaClip(..))
    ));
    assert_eq!(config.normalization, Normalization::Global);
}

#[test]
fn test_validate_valid_config() {
    let config = StackConfig::sigma_clipped(2.5);
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn test_validate_invalid_config_returns_exact_errors() {
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

#[test]
fn test_bias_preset() {
    let config = StackConfig::bias();
    assert!(matches!(
        config.method,
        CombineMethod::Mean(Rejection::Winsorized(c))
            if (c.sigma_low - 3.0).abs() < f32::EPSILON
    ));
    assert_eq!(config.normalization, Normalization::None);
}

#[test]
fn test_dark_preset() {
    let config = StackConfig::dark();
    assert!(matches!(
        config.method,
        CombineMethod::Mean(Rejection::Winsorized(c))
            if (c.sigma_low - 3.0).abs() < f32::EPSILON
    ));
    assert_eq!(config.normalization, Normalization::None);
}

#[test]
fn test_flat_preset() {
    let config = StackConfig::flat();
    assert!(matches!(
        config.method,
        CombineMethod::Mean(Rejection::SigmaClip(c))
            if (c.sigma_low - 3.0).abs() < f32::EPSILON
    ));
    assert_eq!(config.normalization, Normalization::Multiplicative);
}

#[test]
fn test_light_preset() {
    let config = StackConfig::light();
    assert!(matches!(
        config.method,
        CombineMethod::Mean(Rejection::SigmaClip(c))
            if (c.sigma_low - 2.5).abs() < f32::EPSILON
    ));
    assert_eq!(config.weighting, Weighting::Noise);
    assert_eq!(config.normalization, Normalization::Global);
}

#[test]
fn test_gesd_preset_uses_supported_sample_floor() {
    let config = StackConfig::gesd();
    assert!(matches!(
        config.method,
        CombineMethod::Mean(Rejection::Gesd(..))
    ));
    assert_eq!(config.small_n, SmallN::median_below(15));
}
