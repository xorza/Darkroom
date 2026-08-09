use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::math::sum::mean_f32;

use crate::math::statistics::mad_f32_fast;
use crate::stacking::combine::rejection::gesd_config::GesdConfig;
use crate::stacking::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::stacking::combine::rejection::percentile_clip_config::PercentileClipConfig;
use crate::stacking::combine::rejection::sigma_clip_config::SigmaClipConfig;
use crate::stacking::combine::rejection::winsorized_clip_config::{
    WinsorizedClipConfig, WinsorizedEstimate, winsorized_stddev,
};
use crate::stacking::combine::rejection::*;

/// Every rejection config's documented defaults in one place. These were six one-assertion tests
/// whose only difference was which type they named.
#[test]
fn rejection_config_defaults() {
    let sigma = SigmaClipConfig::default();
    assert_eq!(
        (sigma.sigma_low, sigma.sigma_high, sigma.max_iterations),
        (2.5, 2.5, 3)
    );

    let winsorized = WinsorizedClipConfig::default();
    assert_eq!((winsorized.sigma_low, winsorized.sigma_high), (2.5, 2.5));

    let linear_fit = LinearFitClipConfig::default();
    assert_eq!((linear_fit.sigma_low, linear_fit.sigma_high), (3.0, 3.0));

    let percentile = PercentileClipConfig::default();
    assert_eq!(
        (percentile.low_percentile, percentile.high_percentile),
        (10.0, 10.0)
    );
}

/// The symmetric constructor mirrors its one sigma; the asymmetric one keeps both apart.
#[test]
fn sigma_clip_constructors_place_their_arguments() {
    let symmetric = SigmaClipConfig::new(3.0, 5);
    assert_eq!(
        (
            symmetric.sigma_low,
            symmetric.sigma_high,
            symmetric.max_iterations
        ),
        (3.0, 3.0, 5)
    );

    let asymmetric = SigmaClipConfig::new_asymmetric(2.0, 3.0, 5);
    assert_eq!(
        (
            asymmetric.sigma_low,
            asymmetric.sigma_high,
            asymmetric.max_iterations
        ),
        (2.0, 3.0, 5)
    );
}

#[test]
fn test_gesd_config_default() {
    let config = GesdConfig::default();
    assert_eq!(config.alpha, 0.05);
    assert!(config.max_outliers.is_none());

    let automatic_cases = [
        (0, 0),
        (3, 0),
        (4, 1),
        (8, 2),
        (15, 2),
        (24, 2),
        (25, 6),
        (39, 9),
        (40, 10),
        (44, 10),
        (100, 10),
    ];
    for (sample_count, expected) in automatic_cases {
        assert_eq!(
            config.max_outliers_for_size(sample_count),
            expected,
            "automatic limit for {sample_count} samples"
        );
    }

    for (sample_count, configured) in [(15, 3), (44, 11), (100, 25)] {
        assert_eq!(
            GesdConfig::new(0.05, Some(configured)).max_outliers_for_size(sample_count),
            configured,
            "explicit limit for {sample_count} samples"
        );
    }
}

#[test]
fn test_sigma_clip_removes_outlier() {
    let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut scratch());
    let mean = mean_f32(&values[..remaining]);
    assert!(mean < 10.0, "Expected outlier to be clipped, got {}", mean);
    assert!(remaining < 8);
}

#[test]
fn test_sigma_clip_no_outliers() {
    let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0];
    let remaining = SigmaClipConfig::new(3.0, 3).reject(&mut values, &mut scratch());
    assert_eq!(remaining, 5);
}

#[test]
fn test_asymmetric_sigma_clip_removes_high_outlier() {
    let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let remaining =
        SigmaClipConfig::new_asymmetric(4.0, 2.0, 3).reject(&mut values, &mut scratch());
    let mean = mean_f32(&values[..remaining]);
    assert!(mean < 10.0, "High outlier should be clipped, got {}", mean);
    assert!(remaining < 8);
}

#[test]
fn test_asymmetric_sigma_clip_keeps_low_with_high_threshold() {
    // Conservative sigma_low (10.0) + aggressive sigma_high (2.0):
    // high outlier rejected, low outlier kept.
    let mut values = vec![-5.0, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 50.0];
    let remaining =
        SigmaClipConfig::new_asymmetric(10.0, 2.0, 5).reject(&mut values, &mut scratch());

    assert!(
        remaining >= 9,
        "Low outlier should be kept, remaining={}",
        remaining
    );
    let mean = mean_f32(&values[..remaining]);
    assert!(
        mean < 2.5,
        "Mean should be < 2.5 due to kept low outlier, got {}",
        mean
    );
}

#[test]
fn test_sigma_clip_symmetric_equals_asymmetric_same_thresholds() {
    let values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let sigma = 2.5;

    let mut v1 = values.clone();
    let r1 = SigmaClipConfig::new(sigma, 3).reject(&mut v1, &mut scratch());

    let mut v2 = values;
    let r2 = SigmaClipConfig::new_asymmetric(sigma, sigma, 3).reject(&mut v2, &mut scratch());

    assert_eq!(r1, r2);
    assert!((mean_f32(&v1[..r1]) - mean_f32(&v2[..r2])).abs() < 1e-6,);
}

#[test]
fn test_sorted_mad_matches_mad_f32_fast() {
    // `sorted_mad` must reproduce `mad_f32_fast` (the function the sort-once reject replaced)
    // exactly — same upper-middle order statistic of the absolute deviations. Cover odd/even
    // lengths, center inside/outside the data, duplicates, and a heavy outlier.
    let cases: &[&[f32]] = &[
        &[1.0, 2.0, 3.0, 4.0, 100.0],         // odd, outlier
        &[1.0, 2.0, 3.0, 4.0],                // even
        &[-5.0, 0.0, 0.0, 0.0, 1.0, 2.0],     // duplicates at center
        &[10.0, 10.0, 10.0],                  // constant → MAD 0
        &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7], // odd, spread
    ];
    let mut buf = vec![];
    for sorted in cases {
        // Reject always calls it at the median; also probe a couple of off-median centers.
        let mid = sorted[sorted.len() / 2];
        for &center in &[mid, sorted[0], mid + 0.05] {
            let expected = mad_f32_fast(sorted, center, &mut buf);
            let got = sorted_mad(sorted, center);
            assert_eq!(
                got, expected,
                "sorted_mad({sorted:?}, {center}) = {got}, expected {expected}"
            );
        }
    }
}

#[test]
fn test_sigma_clip_survivor_indices_pair_with_values() {
    // After rejection, `indices[..remaining]` must be the original frame indices of the
    // surviving values, i.e. `values[i] == original[indices[i]]`. Regression for the prior
    // quickselect that reordered values without their co-indices, mis-pairing per-frame
    // weights in the noise-weighted (light) combine.
    let original = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let mut values = original.clone();
    let mut sc = scratch();
    let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut sc);

    assert!(
        remaining < original.len(),
        "the 100.0 outlier should be rejected"
    );
    for (i, (&val, &idx)) in values[..remaining]
        .iter()
        .zip(&sc.indices[..remaining])
        .enumerate()
    {
        assert_eq!(
            val, original[idx],
            "survivor {i}: value {val} must equal original[{idx}] = {}",
            original[idx]
        );
    }
    // Frame 7 (value 100.0) is the outlier — its index must not survive.
    assert!(
        !sc.indices[..remaining].contains(&7),
        "rejected outlier's index leaked into the survivors"
    );
}

#[test]
fn test_linear_fit_first_pass_uses_median_mad() {
    // Linear fit's first pass uses median + MAD (same as sigma clip).
    // With max_iterations=1, linear fit behaves identically to a single
    // sigma clip pass.
    let mut values_lf = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
    let mut values_sc = values_lf.clone();

    let lf_remaining = LinearFitClipConfig::new(2.0, 2.0, 1).reject(&mut values_lf, &mut scratch());
    let sc_remaining = SigmaClipConfig::new(2.0, 1).reject(&mut values_sc, &mut scratch());

    // Both should reject the same outlier on the first pass
    assert_eq!(lf_remaining, sc_remaining);
}

#[test]
fn test_winsorized_rejects_outlier() {
    let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let remaining = WinsorizedClipConfig::new(2.0).reject(&mut values, &mut scratch());
    assert!(
        remaining < 8,
        "Outlier should be rejected, got {remaining} survivors"
    );
    let mean = mean_f32(&values[..remaining]);
    assert!(mean < 10.0, "Mean of survivors should be low, got {mean}");
}

#[test]
fn test_linear_fit_constant_data() {
    let mut values = vec![5.0, 5.0, 5.0, 5.0, 5.0];
    let remaining = LinearFitClipConfig::default().reject(&mut values, &mut scratch());
    assert_eq!(remaining, 5);
    assert!((mean_f32(&values[..remaining]) - 5.0).abs() < 0.01);
}

#[test]
fn test_linear_fit_rejects_outlier() {
    let mut values = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
    let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut scratch());
    assert!(remaining < 6);
    assert!(mean_f32(&values[..remaining]) < 20.0);
}

#[test]
fn test_linear_fit_rejects_off_line_point_when_seed_pass_is_clean() {
    // The fit must run even when the median+MAD seed pass rejects nothing — otherwise an
    // off-line point hidden by a steep spread survives. Ramp 10..90 + an off-line `5`:
    // seed median≈45, MAD≈25 → sigma≈37, threshold(2.0)≈74, so the `5` (|Δ|=40) is kept and
    // the seed rejects nothing. The line fit through the sorted values has σ≈0.92 (mean |resid|),
    // threshold(2.0)≈1.84; only `5` (residual≈3.27 from the fitted line) exceeds it.
    let mut values = vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 5.0];
    let mut s = scratch();
    let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut s);

    assert_eq!(remaining, 9, "only the off-line `5` should be rejected");
    assert!(
        !s.indices[..remaining].contains(&9),
        "frame 9 (value 5.0) must be rejected, survivors: {:?}",
        &s.indices[..remaining]
    );
    // Survivors are the clean ramp 10..90 → mean 50.
    let mean = mean_f32(&values[..remaining]);
    assert!((mean - 50.0).abs() < 1e-3, "expected mean 50, got {mean}");
}

#[test]
fn test_sigma_clip_rejects_outlier_in_bright_high_magnitude_data() {
    // Guards the early-exit's numerical soundness (f64 accumulation): on high-magnitude pixels
    // (~8000) with a real outlier, `no_outliers_possible` must not spuriously fire and skip
    // rejection. 14 clean values symmetric about 8000 (mean exactly 8000) + one 9000 outlier.
    let mut values = vec![
        7990.0, 8000.0, 8010.0, 7995.0, 8005.0, 8000.0, 7990.0, 8010.0, 8000.0, 7995.0, 8005.0,
        8000.0, 7990.0, 8010.0, 9000.0,
    ];
    let mut s = scratch();
    let remaining = SigmaClipConfig::new(2.5, 3).reject(&mut values, &mut s);

    assert_eq!(remaining, 14, "the 9000 outlier must be rejected");
    assert!(
        !s.indices[..remaining].contains(&14),
        "frame 14 (value 9000) must be rejected, survivors: {:?}",
        &s.indices[..remaining]
    );
    let mean = mean_f32(&values[..remaining]);
    assert!(
        (mean - 8000.0).abs() < 0.5,
        "expected mean 8000, got {mean}"
    );
}

#[test]
fn test_percentile_clip() {
    let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
    let remaining = PercentileClipConfig::new(20.0, 20.0).reject(&mut values, &mut scratch());
    assert_eq!(remaining, 6);
    // Mean of [3, 4, 5, 6, 7, 8] = 5.5
    assert!((mean_f32(&values[..remaining]) - 5.5).abs() < 0.01);
}

#[test]
fn test_gesd_removes_single_bright_outlier() {
    let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
    let mut s = scratch();
    let remaining = GesdConfig::new(0.05, None).reject(&mut values, &mut s);
    assert_eq!(
        remaining, 7,
        "Exactly the bright outlier should be rejected"
    );
    assert!(
        !s.indices[..remaining].contains(&7),
        "Index 7 (100.0) must be rejected, survivors: {:?}",
        &s.indices[..remaining]
    );
}

#[test]
fn test_gesd_no_outliers() {
    // Constant values — sigma=0 so no outliers detected
    let mut values = vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
    let remaining = GesdConfig::new(0.05, Some(3)).reject(&mut values, &mut scratch());
    assert_eq!(remaining, 8, "No outliers in constant data");
}

#[test]
fn test_gesd_tiny_alpha_uses_finite_limiting_critical_value() {
    let mut values: Vec<f32> = (0..15).map(|value| value as f32).collect();
    let mut scratch = scratch();

    let remaining = GesdConfig::new(f32::MIN_POSITIVE, Some(3)).reject(&mut values, &mut scratch);

    assert_eq!(remaining, 15);
    let expected = 14.0 / 15.0f64.sqrt();
    assert!((scratch.gesd_critical_values[0] - expected).abs() < f64::EPSILON);
}

#[test]
fn test_gesd_keeps_tight_cluster() {
    let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 1.1];
    let remaining = GesdConfig::default().reject(&mut values, &mut scratch());
    assert_eq!(remaining, 8, "Tight cluster should have no rejections");
}

#[test]
fn test_small_sample_handling() {
    // All algorithms should handle n=2 gracefully
    let r = SigmaClipConfig::default().reject(&mut [1.0, 2.0], &mut scratch());
    assert_eq!(r, 2);

    let r = SigmaClipConfig::new_asymmetric(4.0, 3.0, 3).reject(&mut [1.0, 2.0], &mut scratch());
    assert_eq!(r, 2);

    let r = WinsorizedClipConfig::default().reject(&mut [1.0, 2.0], &mut scratch());
    assert_eq!(r, 2);

    let r = LinearFitClipConfig::default().reject(&mut [1.0, 2.0], &mut scratch());
    assert_eq!(r, 2);

    let r = PercentileClipConfig::default().reject(&mut [1.0, 2.0], &mut scratch());
    assert!(r >= 1);

    let r = GesdConfig::default().reject(&mut [1.0, 2.0], &mut scratch());
    assert_eq!(r, 2);
}

#[test]
fn test_sigma_clip_indices_track_survivors() {
    let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let mut s = scratch();
    let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut s);

    let surviving = &s.indices[..remaining];
    assert!(
        !surviving.contains(&7),
        "Frame 7 (outlier) should not survive, survivors: {:?}",
        surviving
    );
    for &idx in surviving {
        assert!(idx < 8, "Invalid surviving index: {}", idx);
    }
}

#[test]
fn test_linear_fit_indices_track_survivors() {
    let mut values = vec![1.0, 2.0, 3.0, 4.0, 100.0, 6.0];
    let mut s = scratch();
    let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut s);

    let surviving = &s.indices[..remaining];
    assert!(
        !surviving.contains(&4),
        "Frame 4 (outlier) should not survive, survivors: {:?}",
        surviving
    );
    for &idx in surviving {
        assert!(idx < 6, "Invalid surviving index: {}", idx);
    }
}

#[test]
fn test_percentile_indices_track_survivors() {
    // Values: [5, 1, 3, 2, 4] → sorted: [1, 2, 3, 4, 5]
    // With 20% clip on each end: clips 1 low, 1 high → survivors [2, 3, 4]
    let mut values = vec![5.0, 1.0, 3.0, 2.0, 4.0];
    let mut s = scratch();
    let remaining = PercentileClipConfig::new(20.0, 20.0).reject(&mut values, &mut s);

    assert_eq!(remaining, 3);
    let surviving = &s.indices[..remaining];
    // Original indices: 5.0→0, 1.0→1, 3.0→2, 2.0→3, 4.0→4
    // Survivors (values 2,3,4) should map to original indices 3, 2, 4
    assert!(
        !surviving.contains(&0) && !surviving.contains(&1),
        "Frames 0 (5.0) and 1 (1.0) should be clipped, survivors: {:?}",
        surviving
    );
    for &idx in surviving {
        assert!(idx < 5, "Invalid surviving index: {}", idx);
    }
}

#[test]
fn test_no_rejection_preserves_all_indices() {
    let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0];
    let mut s = scratch();
    let remaining = SigmaClipConfig::new(3.0, 3).reject(&mut values, &mut s);

    assert_eq!(remaining, 5);
    let surviving = &s.indices[..remaining];
    for i in 0..5 {
        assert!(
            surviving.contains(&i),
            "Index {} should survive when no rejection occurs",
            i
        );
    }
}

#[test]
fn test_rejection_constructors() {
    let r = Rejection::sigma_clip(2.0);
    assert!(
        matches!(r, Rejection::SigmaClip(c) if (c.sigma_low - 2.0).abs() < f32::EPSILON && (c.sigma_high - 2.0).abs() < f32::EPSILON)
    );

    let r = Rejection::winsorized(3.0);
    assert!(
        matches!(r, Rejection::Winsorized(c) if (c.sigma_low - 3.0).abs() < f32::EPSILON && (c.sigma_high - 3.0).abs() < f32::EPSILON)
    );

    let r = Rejection::linear_fit(2.5);
    assert!(matches!(r, Rejection::LinearFit(c)
        if (c.sigma_low - 2.5).abs() < f32::EPSILON && (c.sigma_high - 2.5).abs() < f32::EPSILON));

    let r = Rejection::percentile(15.0);
    assert!(matches!(r, Rejection::Percentile(c)
        if (c.low_percentile - 15.0).abs() < f32::EPSILON && (c.high_percentile - 15.0).abs() < f32::EPSILON));

    let r = Rejection::gesd();
    assert!(matches!(r, Rejection::Gesd(c)
        if (c.alpha - 0.05).abs() < f32::EPSILON && c.max_outliers.is_none()));
}

fn scratch() -> ScratchBuffers {
    ScratchBuffers::default()
}

#[test]
fn test_combine_mean_none() {
    let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let mean = Rejection::None
        .combine_mean(&mut values, &[1.0; 5], &mut scratch(), true)
        .value;
    assert_eq!(mean, 3.0);
}

#[test]
fn test_combine_mean_sigma_clip() {
    let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let mean = Rejection::sigma_clip(2.0)
        .combine_mean(&mut values, &[1.0; 8], &mut scratch(), true)
        .value;
    assert!(mean < 10.0, "Outlier should be clipped, got {}", mean);
}

#[test]
fn test_weighted_percentile_uses_weights() {
    let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
    let weights = vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 10.0, 1.0, 1.0];

    let mean = Rejection::percentile(20.0)
        .combine_mean(&mut values, &weights, &mut scratch(), true)
        .value;

    assert!(
        mean > 5.5 + 0.5,
        "Weighted percentile should be pulled toward heavily weighted value 8, got {}",
        mean
    );
}

#[test]
fn test_weighted_winsorized_uses_weights() {
    let mut values = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
    let weights = vec![10.0, 1.0, 1.0, 1.0, 1.0, 1.0];

    let mean = Rejection::winsorized(2.0)
        .combine_mean(&mut values, &weights, &mut scratch(), true)
        .value;

    let mut values_unwt = vec![1.0, 2.0, 2.0, 2.0, 2.0, 100.0];
    let uniform_weights = vec![1.0; 6];
    let unweighted_mean = Rejection::winsorized(2.0)
        .combine_mean(&mut values_unwt, &uniform_weights, &mut scratch(), true)
        .value;

    assert!(
        mean < unweighted_mean,
        "Weighted winsorized (heavy on 1.0) should be less than uniform: {} vs {}",
        mean,
        unweighted_mean,
    );
}

#[test]
fn test_weighted_asymmetric_sigma_clip() {
    let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let weights = vec![10.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];

    let mean = Rejection::sigma_clip_asymmetric(4.0, 2.0)
        .combine_mean(&mut values, &weights, &mut scratch(), true)
        .value;

    assert!(mean < 2.5, "Should be pulled toward 1.0, got {}", mean);
}

#[test]
fn test_weighted_sigma_clip_weight_alignment() {
    let mut values = vec![2.0, 100.0, 3.0, 2.5, 2.2, 1.8, 2.8, 2.3];
    let weights = vec![10.0, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1];

    let mean = Rejection::sigma_clip(2.0)
        .combine_mean(&mut values, &weights, &mut scratch(), true)
        .value;

    assert!(
        (mean - 2.0).abs() < 0.25,
        "Weighted mean should be ~2.0 (dominated by frame 0, weight=10.0), got {}",
        mean
    );
}

#[test]
fn test_weighted_linear_fit_weight_alignment() {
    // Tight cluster [1.0, 1.1, 1.2, 1.3, 1.4] with outlier 100.0
    // After rejection removes 100, weighted mean dominated by frame 0 (weight=10)
    let mut values = vec![1.0, 1.1, 1.2, 1.3, 100.0, 1.4];
    let weights = vec![10.0, 0.1, 0.1, 0.1, 0.1, 0.1];

    let mean = Rejection::linear_fit(3.0)
        .combine_mean(&mut values, &weights, &mut scratch(), true)
        .value;

    assert!(
        mean < 1.1,
        "Weighted mean should be pulled toward frame 0 (value=1.0, weight=10.0), got {}",
        mean
    );
}

#[test]
fn test_weighted_gesd_weight_alignment() {
    let mut values = vec![1.0, 1.1, 0.9, 1.0, 1.2, 0.8, 1.0, 100.0];
    let weights = vec![10.0, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1];

    let mean = Rejection::Gesd(GesdConfig::new(0.05, Some(3)))
        .combine_mean(&mut values, &weights, &mut scratch(), true)
        .value;

    assert!(
        (mean - 1.0).abs() < 0.05,
        "Weighted mean should be ~1.0 (dominated by frame 0, weight=10.0), got {}",
        mean
    );
}

#[test]
fn test_gesd_matches_nist_reference_example() {
    let mut values = vec![
        -0.25, 0.68, 0.94, 1.15, 1.20, 1.26, 1.26, 1.34, 1.38, 1.43, 1.49, 1.49, 1.55, 1.56, 1.58,
        1.65, 1.69, 1.70, 1.76, 1.77, 1.81, 1.91, 1.94, 1.96, 1.99, 2.06, 2.09, 2.10, 2.14, 2.15,
        2.23, 2.24, 2.26, 2.35, 2.37, 2.40, 2.47, 2.54, 2.62, 2.64, 2.90, 2.92, 2.92, 2.93, 3.21,
        3.26, 3.30, 3.59, 3.68, 4.30, 4.64, 5.34, 5.42, 6.01,
    ];
    let expected = [
        (3.118, 3.158),
        (2.942, 3.151),
        (3.179, 3.143),
        (2.810, 3.136),
        (2.815, 3.128),
        (2.848, 3.120),
        (2.279, 3.111),
        (2.310, 3.103),
        (2.101, 3.094),
        (2.067, 3.085),
    ];
    let mut scratch = scratch();

    let remaining = GesdConfig::new(0.05, Some(10)).reject(&mut values, &mut scratch);

    assert_eq!(remaining, 51);
    assert_eq!(
        scratch.indices[..remaining]
            .iter()
            .filter(|&&index| index >= 51)
            .count(),
        0
    );
    for ((statistic, critical), (expected_statistic, expected_critical)) in scratch
        .gesd_statistics
        .iter()
        .zip(&scratch.gesd_critical_values)
        .zip(expected)
    {
        assert!(
            (statistic - expected_statistic).abs() <= 0.0015,
            "expected statistic {expected_statistic}, got {statistic}"
        );
        assert!(
            (critical - expected_critical).abs() <= 0.0015,
            "expected critical value {expected_critical}, got {critical}"
        );
    }
}

#[test]
fn test_gesd_is_sign_symmetric_for_asymmetric_outliers() {
    let values = vec![
        -1.4, -1.2, -1.0, -0.8, -0.6, -0.4, -0.2, 0.0, 0.2, 0.4, 0.6, 0.8, 1.0, 1.2, 1.4, -8.0,
        10.0,
    ];
    let mut original = values.clone();
    let mut mirrored: Vec<f32> = values.iter().map(|value| -value).collect();
    let config = GesdConfig::new(0.05, Some(2));
    let mut original_scratch = scratch();
    let mut mirrored_scratch = scratch();

    let original_remaining = config.reject(&mut original, &mut original_scratch);
    let mirrored_remaining = config.reject(&mut mirrored, &mut mirrored_scratch);

    assert_eq!(original_remaining, 15);
    assert_eq!(mirrored_remaining, 15);
    let mut original_survivors = original_scratch.indices[..original_remaining].to_vec();
    let mut mirrored_survivors = mirrored_scratch.indices[..mirrored_remaining].to_vec();
    original_survivors.sort_unstable();
    mirrored_survivors.sort_unstable();
    assert_eq!(original_survivors, mirrored_survivors);
    assert_eq!(original_survivors, (0..15).collect::<Vec<_>>());
}

#[test]
fn test_gesd_gaussian_false_positive_rate_matches_alpha() {
    const ALPHA: f32 = 0.05;
    const TRIALS: usize = 4_000;

    // Not `TestRng`: it is an LCG, and Box-Muller over consecutive LCG outputs lays the pairs
    // on a handful of spirals rather than filling the plane. That distorts the tails, which is
    // exactly what a GESD outlier test measures — swapping this generator in moves the observed
    // false-positive rate from 0.050 to 0.076, past the 5-sigma bound below.
    let mut rng = ChaCha8Rng::seed_from_u64(0x947e_4d3a_7c16_b205);
    for sample_count in [15, 25, 50, 100] {
        let config = GesdConfig::new(ALPHA, Some(sample_count / 4));
        let mut scratch = scratch();
        let mut false_positives = 0usize;

        for _ in 0..TRIALS {
            let mut values: Vec<f32> = (0..sample_count)
                .map(|_| standard_normal(&mut rng))
                .collect();
            if config.reject(&mut values, &mut scratch) < sample_count {
                false_positives += 1;
            }
        }

        let actual = false_positives as f64 / TRIALS as f64;
        let expected = f64::from(ALPHA);
        let standard_error = (expected * (1.0 - expected) / TRIALS as f64).sqrt();
        assert!(
            (actual - expected).abs() <= 5.0 * standard_error,
            "n={sample_count}: expected false-positive rate {expected}, got {actual}"
        );
    }
}

fn standard_normal(rng: &mut ChaCha8Rng) -> f32 {
    let u1 = rng.random::<f64>().max(f64::MIN_POSITIVE);
    let u2 = rng.random::<f64>();
    ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
}

#[test]
fn test_rejection_default_is_sigma_clip() {
    let r = Rejection::default();
    assert!(
        matches!(r, Rejection::SigmaClip(c) if (c.sigma_low - 2.5).abs() < f32::EPSILON
            && (c.sigma_high - 2.5).abs() < f32::EPSILON
            && c.max_iterations == 3)
    );
}

#[test]
fn test_sigma_clip_multiple_outliers() {
    let mut values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 50.0, 80.0, 100.0];
    let remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values, &mut scratch());
    // All three outliers should be removed
    for &v in &values[..remaining] {
        assert!(v < 10.0, "Outlier {} should have been clipped", v);
    }
    assert!(
        remaining <= 5,
        "Expected at most 5 survivors, got {}",
        remaining
    );
}

#[test]
fn test_winsorized_no_outliers() {
    let mut values = vec![2.0, 2.1, 2.2, 1.9, 2.0];
    let remaining = WinsorizedClipConfig::new(3.0).reject(&mut values, &mut scratch());
    assert_eq!(remaining, 5, "No values should be rejected");
}

#[test]
fn test_winsorized_asymmetric() {
    // Strong high outlier, mild low variation
    let mut values = vec![1.0, 1.1, 1.2, 0.9, 1.0, 50.0];
    let remaining =
        WinsorizedClipConfig::new_asymmetric(3.0, 2.0).reject(&mut values, &mut scratch());
    assert!(remaining < 6, "High outlier should be rejected");
    let mean = mean_f32(&values[..remaining]);
    assert!(mean < 5.0, "Mean without outlier should be low, got {mean}");
}

#[test]
fn test_linear_fit_rejects_extreme_outlier() {
    // Linear fit uses fit-derived sigma which is tighter than median+MAD.
    // Initial pass (median+MAD) removes the gross outlier, then the fit
    // refines sigma. With max_iterations=1, only the initial pass runs.
    let mut values = vec![10.0, 10.5, 11.0, 10.2, 10.8, 10.3, 10.7, 50.0];
    let mut s = scratch();
    let remaining = LinearFitClipConfig::new(3.0, 3.0, 1).reject(&mut values, &mut s);
    assert_eq!(remaining, 7, "Only the outlier should be rejected");
    let surviving = &s.indices[..remaining];
    assert!(
        !surviving.contains(&7),
        "Frame 7 (outlier 50.0) should not survive, survivors: {:?}",
        surviving
    );
}

#[test]
fn test_linear_fit_tighter_than_sigma_clip() {
    // Linear fit derives sigma from residuals of a linear fit through sorted
    // values. For well-distributed data, this sigma is tighter than median+MAD,
    // so linear fit rejects more aggressively on subsequent iterations.
    let mut values_lf = vec![1.0, 3.0, 5.0, 7.0, 50.0, 11.0, 13.0, 15.0];
    let mut values_sc = values_lf.clone();

    let lf_remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values_lf, &mut scratch());
    let sc_remaining = SigmaClipConfig::new(2.0, 3).reject(&mut values_sc, &mut scratch());

    // Linear fit should reject more aggressively than sigma clip
    assert!(
        lf_remaining <= sc_remaining,
        "Linear fit (remaining={}) should be at least as aggressive as sigma clip (remaining={})",
        lf_remaining,
        sc_remaining
    );
}

#[test]
fn test_surviving_range_single_element() {
    let config = PercentileClipConfig::new(10.0, 10.0);
    let range = config.surviving_range(1);
    assert_eq!(range, 0..1, "Single element must survive");
}

#[test]
fn test_surviving_range_extreme_percentiles() {
    // 49% + 49% = 98% clipped — should still keep at least 1
    let config = PercentileClipConfig::new(49.0, 49.0);
    let range = config.surviving_range(5);
    assert!(!range.is_empty(), "Must keep at least one element");
    // For n=5: low_count = floor(0.49*5) = 2, high_count = floor(0.49*5) = 2
    // start=2, end=5-2=3, range = 2..3 (1 element)
    assert_eq!(range.len(), 1);
}

#[test]
fn test_weighted_mean_indexed_basic() {
    // values [2, 4, 6] with weights [10, 1, 1] via identity indices
    // expected: (20 + 4 + 6) / 12 = 2.5
    let values = [2.0, 4.0, 6.0];
    let weights = [10.0, 1.0, 1.0];
    let indices = [0, 1, 2];
    let mut buf = Vec::new();
    let mean = weighted_mean_indexed(&values, &weights, &indices, &mut buf);
    assert!((mean - 2.5).abs() < 1e-6, "Expected 2.5, got {}", mean);
}

#[test]
fn test_weighted_mean_indexed_reordered() {
    // Simulate rejection reordering: values were [10, 99, 20] → after rejecting idx 1,
    // survivors are values [10, 20] with indices [0, 2]
    let values = [10.0, 20.0];
    let weights = [5.0, 0.5, 1.0]; // original weights for 3 frames
    let indices = [0, 2]; // frame 0 and frame 2 survived
    let mut buf = Vec::new();
    let mean = weighted_mean_indexed(&values, &weights, &indices, &mut buf);
    // expected: (10*5 + 20*1) / (5+1) = 70/6 ≈ 11.667
    assert!(
        (mean - 70.0 / 6.0).abs() < 1e-5,
        "Expected {}, got {}",
        70.0 / 6.0,
        mean
    );
}

#[test]
fn test_combine_mean_percentile_unweighted() {
    let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
    let ones = vec![1.0f32; values.len()];
    let mean = Rejection::percentile(20.0)
        .combine_mean(&mut values, &ones, &mut scratch(), true)
        .value;
    // Clips 2 low (1,2) and 2 high (9,10), mean of [3,4,5,6,7,8] = 5.5
    assert!(
        (mean - 5.5).abs() < 0.01,
        "Unweighted percentile mean should be 5.5, got {}",
        mean
    );
}

#[test]
fn test_combine_mean_none_with_weights() {
    let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let weights = vec![10.0, 1.0, 1.0, 1.0, 1.0];
    let mean = Rejection::None
        .combine_mean(&mut values, &weights, &mut scratch(), true)
        .value;
    // Weighted mean: (10+2+3+4+5) / (10+1+1+1+1) = 24/14 ≈ 1.714
    assert!(
        (mean - 24.0 / 14.0).abs() < 1e-5,
        "Weighted mean with no rejection should be {}, got {}",
        24.0 / 14.0,
        mean
    );
}

#[test]
fn unit_weights_reduce_to_the_plain_mean_of_the_survivors() {
    // Calibration masters used to reach a separate reducer that took `mean_f32` over the
    // survivors; they now go through this weighted reduction with unit weights. The two must
    // agree bit-for-bit or merging the engines silently changed every master ever built.
    let values = vec![1.0f32, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 100.0];
    let ones = vec![1.0f32; 8];

    let mut weighted_values = values.clone();
    let combined =
        Rejection::sigma_clip(2.0).combine_mean(&mut weighted_values, &ones, &mut scratch(), true);

    // The retired path, reproduced: reject, then plainly average what survived.
    let mut plain_values = values;
    let mut plain_scratch = scratch();
    let remaining = Rejection::sigma_clip(2.0).reject(&mut plain_values, &mut plain_scratch);
    let plain = mean_f32(&plain_values[..remaining]);

    assert!(remaining < 8, "the 100.0 outlier must be rejected");
    assert_eq!(combined.survivor_count, remaining);
    assert_eq!(
        combined.value.to_bits(),
        plain.to_bits(),
        "unit-weighted reduction {} diverged from the plain survivor mean {}",
        combined.value,
        plain
    );
}

#[test]
fn test_winsorized_robust_estimate_uses_stddev_not_mad() {
    // With known data, verify robust_estimate returns stddev-based sigma
    // (not MAD-based). For Gaussian data, stddev > MAD * 1.4826 is false,
    // but for uniform-like data they differ noticeably.
    let config = WinsorizedClipConfig::new(3.0);
    let values: Vec<f32> = (0..20).map(|i| 10.0 + i as f32 * 0.1).collect();
    let mut working = vec![];
    let WinsorizedEstimate { center, sigma } = config.robust_estimate(&values, &mut working);

    // Center should be near median (10.95)
    assert!(
        (center - 10.95).abs() < 0.2,
        "Center should be near median, got {center}"
    );
    // Sigma should be positive and reasonable (1.134 * stddev of uniform-ish data)
    assert!(sigma > 0.0, "Sigma should be positive, got {sigma}");
    assert!(
        sigma < 2.0,
        "Sigma should be reasonable for tight data, got {sigma}"
    );
}

#[test]
fn test_winsorized_correction_factor_applied() {
    // Verify 1.134 correction is applied by comparing with raw stddev
    let config = WinsorizedClipConfig::new(3.0);
    let values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
    let mut working = vec![];
    let sigma = config.robust_estimate(&values, &mut working).sigma;

    // Raw stddev of 1..=10 is ~3.03. With no outliers to Winsorize,
    // sigma should be approximately 3.03 * 1.134 ≈ 3.43
    let raw_stddev = winsorized_stddev(&values, 5.5);
    let expected = raw_stddev * 1.134;
    assert!(
        (sigma - expected).abs() < 0.5,
        "Sigma {sigma} should be near {expected} (raw_stddev {raw_stddev} * 1.134)"
    );
}

#[test]
fn test_winsorized_converges() {
    // With a clear outlier, verify convergence produces stable estimates
    let config = WinsorizedClipConfig::new(2.5);
    let values = vec![10.0, 10.1, 10.2, 9.9, 10.0, 10.1, 9.8, 10.3, 50.0];
    let mut working = vec![];
    let WinsorizedEstimate { center, sigma } = config.robust_estimate(&values, &mut working);

    // Center should be near the cluster (~10.05), not pulled toward 50
    assert!(
        (center - 10.05).abs() < 0.5,
        "Center should be near 10.05, got {center}"
    );
    // Sigma should reflect the cluster spread, not the outlier
    assert!(
        sigma < 2.0,
        "Sigma should be small (cluster spread), got {sigma}"
    );
}

#[test]
fn test_winsorized_huber_constant_not_user_sigma() {
    // Verify that Winsorization boundaries use c=1.5, not user's sigma.
    // With sigma=10.0 (very permissive), phase 1 should still use c=1.5
    // for Winsorization, producing the same robust estimates.
    let config_permissive = WinsorizedClipConfig::new(10.0);
    let config_tight = WinsorizedClipConfig::new(2.0);

    // Use a mild outlier (~5σ from center) so tight rejects but permissive keeps.
    // Clean cluster ~1.0 (stddev ~0.15, corrected ~0.17), outlier at 2.0 is ~5.6σ.
    let values = vec![1.0, 1.1, 1.2, 0.9, 1.0, 1.1, 0.8, 1.3, 2.0];
    let mut w1 = vec![];
    let mut w2 = vec![];
    let WinsorizedEstimate {
        center: center1,
        sigma: sigma1,
    } = config_permissive.robust_estimate(&values, &mut w1);
    let WinsorizedEstimate {
        center: center2,
        sigma: sigma2,
    } = config_tight.robust_estimate(&values, &mut w2);

    // Both should produce the same robust estimates (same Huber c=1.5)
    assert!(
        (center1 - center2).abs() < 1e-4,
        "Centers should match: {center1} vs {center2}"
    );
    assert!(
        (sigma1 - sigma2).abs() < 1e-4,
        "Sigmas should match: {sigma1} vs {sigma2}"
    );

    // But rejection results should differ (permissive keeps outlier)
    let mut v1 = values.clone();
    let mut v2 = values.clone();
    let r1 = config_permissive.reject(&mut v1, &mut scratch());
    let r2 = config_tight.reject(&mut v2, &mut scratch());
    assert!(
        r1 > r2,
        "Permissive sigma should keep more values: {r1} vs {r2}"
    );
}

#[test]
fn test_linear_fit_per_pixel_rejection() {
    // Construct data with a clear linear trend plus one outlier.
    // Sorted values: [1, 2, 3, 4, 5, 6, 7, 100]
    // The fit through sorted positions should approximate y = 1 + x.
    // Value 100 at position 7 has fitted value ~8, residual ~92.
    // With per-pixel rejection, this should be caught easily.
    // With single-center rejection (old bug), center ≈ fit(3.5) ≈ 4.5,
    // values 1 and 7 would be far from center too.
    let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 100.0];
    let mut s = scratch();
    let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut s);

    // The outlier (100.0) should be rejected
    assert!(
        remaining == 7,
        "Should reject only the outlier, got {remaining} survivors"
    );
    // All survivors should be in range [1, 7]
    for &v in &values[..remaining] {
        assert!((1.0..=7.0).contains(&v), "Unexpected survivor: {v}");
    }
}

#[test]
fn test_linear_fit_sigma_is_mean_abs_dev() {
    // For a perfect linear sequence, mean absolute deviation from fit should be ~0.
    // Adding a known deviation: values = [1, 2, 3, 4, 5] + noise on last.
    // After initial median+MAD pass (no rejection for clean data),
    // the fit pass should compute sigma from mean abs dev.
    let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut scratch());
    // Perfect line: no rejections
    assert_eq!(remaining, 5, "Perfect line should have no rejections");
}

#[test]
fn test_linear_fit_preserves_trend() {
    // Linear fit should NOT reject values that follow a trend, even if
    // they're far from the median. This was the old bug: single-center
    // rejection would reject endpoints of a steep trend.
    let mut values = vec![1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0];
    let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut scratch());
    assert_eq!(
        remaining, 8,
        "All values follow a linear trend — none should be rejected"
    );
}

#[test]
fn test_linear_fit_rejects_middle_outlier() {
    // An outlier in the middle of the distribution should be caught by
    // per-pixel rejection. After sorting: [1, 2, 3, 4, 5, 50, 6, 7]
    // → sorted: [1, 2, 3, 4, 5, 6, 7, 50]. Fit: ~y = 0.71 + 0.71*x.
    // Fitted value at position 7 ≈ 5.7, residual of 50 ≈ 44.3.
    let mut values = vec![1.0, 2.0, 3.0, 50.0, 5.0, 6.0, 7.0, 4.0];
    let mut s = scratch();
    let remaining = LinearFitClipConfig::new(2.0, 2.0, 3).reject(&mut values, &mut s);
    assert!(remaining < 8, "Outlier 50 should be rejected");
    for &v in &values[..remaining] {
        assert!(v < 10.0, "Outlier should not survive, got {v}");
    }
}

#[test]
fn test_sort_with_indices_large_n_correctness() {
    // 100 elements in reverse order → exercises the introsort path (threshold=64).
    // After sorting: values should be [0, 1, 2, ..., 99] and indices should
    // map each sorted position back to its original position.
    // Original: values[0]=99, values[1]=98, ..., values[99]=0
    // Sorted:   values[0]=0, values[1]=1, ..., values[99]=99
    // Indices:  indices[0]=99, indices[1]=98, ..., indices[99]=0
    let n = 100;
    let mut values: Vec<f32> = (0..n).rev().map(|i| i as f32).collect();
    let mut scratch = scratch();
    scratch.reset_indices(n);
    scratch.sort_with_indices(&mut values, n);

    for (i, (&value, &index)) in values.iter().zip(&scratch.indices).enumerate() {
        assert_eq!(value, i as f32, "values[{i}] wrong");
        // Original position of value i was (n-1-i)
        assert_eq!(index, n - 1 - i, "indices[{i}] wrong");
    }
}

#[test]
fn test_sort_with_indices_large_n_shuffled() {
    // Deterministic shuffle: positions generated by (i*37) % 200.
    // Verifies sort + index tracking for a non-trivial permutation.
    let n = 200;
    let mut values = vec![0.0f32; n];
    // Place value (i*37 % 200) at position i
    for (i, v) in values.iter_mut().enumerate() {
        *v = ((i * 37) % n) as f32;
    }
    let original_values = values.clone();
    let mut scratch = scratch();
    scratch.reset_indices(n);
    scratch.sort_with_indices(&mut values, n);

    // Values must be sorted
    for i in 1..n {
        assert!(
            values[i - 1] <= values[i],
            "Not sorted at {}: {} > {}",
            i,
            values[i - 1],
            values[i]
        );
    }
    // Each index must point back to where this value came from
    for (i, (&v, &idx)) in values.iter().zip(scratch.indices.iter()).enumerate() {
        assert_eq!(
            original_values[idx], v,
            "Index tracking broken at position {i}: indices[{i}]={idx}, original[{idx}]={}, but values[{i}]={v}",
            original_values[idx]
        );
    }
}

#[test]
fn test_percentile_large_stack() {
    // 100 frames (exercises introsort path). Values 0..99.
    // 10% clip each end → remove bottom 10 and top 10 → survivors 10..90 (80 values).
    // Indices should map survivors to their original positions.
    let n = 100;
    // Reverse order to test sorting
    let mut values: Vec<f32> = (0..n).rev().map(|i| i as f32).collect();
    let mut s = scratch();
    s.indices.resize(n, 0);

    let remaining = PercentileClipConfig::new(10.0, 10.0).reject(&mut values, &mut s);

    assert_eq!(remaining, 80);
    // Surviving values should be 10..90 (sorted)
    for (i, &v) in values[..80].iter().enumerate() {
        assert_eq!(v, (i + 10) as f32, "Surviving value at position {i}");
    }
    // Surviving indices should map back: value 10 was at original position 89, etc.
    for (i, (&v, &idx)) in values[..80].iter().zip(s.indices[..80].iter()).enumerate() {
        let expected_original_pos = n - 1 - (i + 10);
        assert_eq!(
            idx, expected_original_pos,
            "Index at position {i}: value {v} came from original position {expected_original_pos}"
        );
    }
}

#[test]
fn test_linear_fit_large_stack() {
    // 100 frames with a linear trend (value = index) plus one outlier.
    // values[50] = 1000.0 (original position 50). Should be rejected.
    // All other values should survive.
    let n = 100;
    let mut values: Vec<f32> = (0..n).map(|i| i as f32).collect();
    values[50] = 1000.0;
    let mut s = scratch();

    let remaining = LinearFitClipConfig::new(3.0, 3.0, 3).reject(&mut values, &mut s);

    assert_eq!(
        remaining, 99,
        "Only the outlier at position 50 should be rejected"
    );
    // The outlier (1000.0) must not be among survivors
    for &v in &values[..remaining] {
        assert!(v < 100.0, "Outlier 1000.0 should not survive, got {v}");
    }
    // Original frame index 50 must not appear in survivors
    assert!(
        !s.indices[..remaining].contains(&50),
        "Frame 50 (the outlier) should be rejected"
    );
}

#[test]
fn test_reset_indices_basic() {
    let mut scratch = ScratchBuffers::default();
    scratch.reset_indices(5);
    let indices = &scratch.indices;
    assert_eq!(*indices, vec![0, 1, 2, 3, 4]);
}

#[test]
fn test_reset_indices_reuses_allocation() {
    let mut scratch = ScratchBuffers::default();
    scratch.indices.reserve(100);
    scratch.reset_indices(5);
    let indices = &scratch.indices;
    assert_eq!(*indices, vec![0, 1, 2, 3, 4]);
    assert!(indices.capacity() >= 100, "should preserve allocation");

    // Reset to different size — should reuse existing allocation
    scratch.reset_indices(3);
    let indices = &scratch.indices;
    assert_eq!(*indices, vec![0, 1, 2]);
    assert!(indices.capacity() >= 100);
}

#[test]
fn test_reset_indices_overwrites_stale_data() {
    let mut scratch = ScratchBuffers {
        indices: vec![99, 88, 77, 66, 55],
        ..Default::default()
    };
    scratch.reset_indices(5);
    let indices = &scratch.indices;
    assert_eq!(*indices, vec![0, 1, 2, 3, 4]);
}

#[test]
fn test_reset_indices_empty() {
    let mut scratch = ScratchBuffers {
        indices: vec![1, 2, 3],
        ..Default::default()
    };
    scratch.reset_indices(0);
    let indices = &scratch.indices;
    assert!(indices.is_empty());
}

#[test]
fn test_no_outliers_possible_tight_cluster() {
    // 20 values all equal to 10.0 → stddev=0 → returns true
    let values = vec![10.0f32; 20];
    assert!(SigmaClipConfig::no_outliers_possible(&values, 2.5));
}

#[test]
fn test_no_outliers_possible_small_spread() {
    // values = [10, 10, 10, ..., 10, 11, 9] (18×10 + 11 + 9), N=20
    // trimmed (exclude min=9, max=11): 18×10 + one of {9,11} excluded
    // Actually exclude the single min (9) and single max (11):
    //   trimmed = 18×10.0 = 180, trimmed_n = 18, trimmed_mean = 10.0
    //   trimmed variance: 18 × (10-10)² / 17 = 0
    //   stddev = 0 → returns true
    let mut values = vec![10.0f32; 18];
    values.push(11.0);
    values.push(9.0);
    assert!(SigmaClipConfig::no_outliers_possible(&values, 2.5));
}

#[test]
fn test_no_outliers_possible_clear_outlier() {
    // 17×10.0 + [9.0, 11.0, 100.0], N=20
    // min=9.0, max=100.0, excluded from trimmed stats.
    // trimmed: 17×10.0 + 11.0 = 181, trimmed_n=18, trimmed_mean=181/18 ≈ 10.056
    // trimmed sum_sq = 17×100 + 121 = 1821
    // trimmed var = (1821 - 181²/18) / 17 = (1821 - 1820.056) / 17 ≈ 0.056
    // trimmed stddev ≈ 0.236
    // max_dev = |100.0 - 10.056| = 89.94
    // threshold = 2.5 × 0.236 = 0.59
    // 89.94 > 0.59 → returns false (outlier detected)
    let mut values = vec![10.0f32; 17];
    values.extend([9.0, 11.0, 100.0]);
    assert!(!SigmaClipConfig::no_outliers_possible(&values, 2.5));
}

#[test]
fn test_no_outliers_possible_returns_false_for_small_n() {
    // N < 10 always returns false (trimming would distort too much)
    let values = vec![10.0f32; 5];
    assert!(!SigmaClipConfig::no_outliers_possible(&values, 2.5));

    let values = vec![10.0f32; 9];
    assert!(!SigmaClipConfig::no_outliers_possible(&values, 2.5));
}

#[test]
fn test_no_outliers_possible_moderate_spread() {
    // Linearly spaced: [0, 1, 2, ..., 19], N=20
    // min=0, max=19, excluded → trimmed = [1..18], trimmed_n=18
    // trimmed_sum = 1+2+...+18 = 171, trimmed_mean = 171/18 = 9.5
    // trimmed_sum_sq = 1+4+9+...+324 = 2109
    // trimmed_var = (2109 - 171²/18) / 17 = (2109 - 1624.5) / 17 = 484.5/17 ≈ 28.5
    // trimmed_stddev ≈ 5.34
    // max_dev = max(|0 - 9.5|, |19 - 9.5|) = 9.5
    // threshold = 2.5 × 5.34 = 13.35
    // 9.5 < 13.35 → returns true (no outlier exceeds threshold)
    let values: Vec<f32> = (0..20).map(|i| i as f32).collect();
    assert!(SigmaClipConfig::no_outliers_possible(&values, 2.5));
}

#[test]
fn test_no_outliers_possible_asymmetric_outliers() {
    // [1, 1.5, 2, 2.5, 3, 50, 80, 100, 1, 1] — N=10
    // min=1.0, max=100.0, excluded
    // trimmed: [1.5, 2, 2.5, 3, 50, 80, 1, 1] — n=8
    // trimmed_sum = 141, trimmed_mean = 17.625
    // Outlier 80 is still in trimmed set → large stddev
    // max_dev = |100 - 17.625| = 82.375
    // Should return false (outliers present)
    let values = vec![1.0, 1.5, 2.0, 2.5, 3.0, 50.0, 80.0, 100.0, 1.0, 1.0];
    assert!(!SigmaClipConfig::no_outliers_possible(&values, 2.0));
}

#[test]
fn test_no_outliers_possible_does_not_break_rejection() {
    // End-to-end: early exit must not prevent correct rejection.
    // Need data with non-zero MAD so sigma clipping can define a threshold.
    // 47 values at 9.0 + 47 values at 11.0 + 6 outliers = 100 values.
    // median = 11 (or 9, depending on order), MAD ≈ 1, sigma ≈ 1.48
    // threshold = 2.5 × 1.48 = 3.7 → outliers at 100+ are clearly rejected.
    let mut values: Vec<f32> = vec![9.0; 47];
    values.extend(vec![11.0; 47]);
    values.extend([100.0, 200.0, 500.0, 600.0, 700.0, 800.0]);
    let remaining = SigmaClipConfig::new(2.5, 3).reject(&mut values, &mut scratch());
    // All 6 large outliers must be rejected
    for &v in &values[..remaining] {
        assert!(v < 20.0, "Outlier {v} should have been clipped");
    }
    assert_eq!(remaining, 94);
}

#[test]
fn test_no_outliers_possible_clean_data_skips_quickselect() {
    // 100×10.0 (perfectly uniform) — early exit should trigger,
    // meaning reject returns all values with no changes.
    let mut values = vec![10.0f32; 100];
    let remaining = SigmaClipConfig::new(2.5, 3).reject(&mut values, &mut scratch());
    assert_eq!(remaining, 100);
    // All values unchanged
    for &v in &values {
        assert_eq!(v, 10.0);
    }
}

#[test]
fn test_weighted_mean_indexed_all_zero_weights() {
    // All weights zero → should return 0.0, not NaN/Inf
    let values = [5.0f32, 10.0, 15.0];
    let weights = [0.0f32, 0.0, 0.0];
    let indices = [0, 1, 2];
    let mut buf = Vec::new();
    let result = weighted_mean_indexed(&values, &weights, &indices, &mut buf);
    assert!(
        (result - 0.0).abs() < 1e-6,
        "Should return 0.0, got {}",
        result
    );
}

#[test]
fn test_weighted_mean_indexed_partial_zero_weights() {
    // values=[5, 10, 15], weights=[0, 2, 0], indices=[0, 1, 2]
    // Only middle value has nonzero weight → mean = 10*2 / 2 = 10.0
    let values = [5.0f32, 10.0, 15.0];
    let weights = [0.0f32, 2.0, 0.0];
    let indices = [0, 1, 2];
    let mut buf = Vec::new();
    let result = weighted_mean_indexed(&values, &weights, &indices, &mut buf);
    assert!((result - 10.0).abs() < 1e-6);
}

#[test]
fn weighted_mean_indexed_preserves_small_increments() {
    // 0.5 sits below half the ULP of 2e7, so a naive f32 accumulation would
    // drop every increment; the wider/compensated weighted mean recovers them.
    // Weights are all 1.0, so the result is a plain mean.
    let mut values = vec![0.5_f32; 17];
    values[0] = 2.0e7;
    let weights = vec![1.0_f32; values.len()];
    let indices: Vec<usize> = (0..values.len()).collect();

    let mut buf = Vec::new();
    let mean = weighted_mean_indexed(&values, &weights, &indices, &mut buf);

    // True mean = (2e7 + 16*0.5) / 17 = 20_000_008 / 17 ≈ 1_176_471.06.
    // A naive f32 sum gives ~2e7/17 ≈ 1_176_470.59, off by 8/17 ≈ 0.47.
    let expected = (2.0e7_f64 + 8.0) / 17.0;
    assert!(
        (mean as f64 - expected).abs() < 0.1,
        "precise mean {mean} must be within 0.1 of {expected} (naive loses ~0.47)"
    );
}
