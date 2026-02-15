//! Tests for statistical functions.

use super::*;

// ---------------------------------------------------------------------------
// Median tests
// ---------------------------------------------------------------------------

#[test]
fn test_median_odd() {
    let mut values = [1.0f32, 3.0, 2.0, 5.0, 4.0];
    assert!((median_f32_mut(&mut values) - 3.0).abs() < f32::EPSILON);
}

#[test]
fn test_median_even() {
    let mut values = [1.0f32, 2.0, 3.0, 4.0];
    assert!((median_f32_mut(&mut values) - 2.5).abs() < f32::EPSILON);
}

#[test]
fn test_median_two_elements() {
    let mut values = [1.0f32, 5.0];
    assert!((median_f32_mut(&mut values) - 3.0).abs() < f32::EPSILON);
}

#[test]
fn test_median_f32_single() {
    let mut values = [42.0f32];
    assert!((median_f32_mut(&mut values) - 42.0).abs() < f32::EPSILON);
}

#[test]
fn test_median_f32_negative() {
    let mut values = [-5.0f32, -3.0, -1.0, 2.0, 4.0];
    assert!((median_f32_mut(&mut values) - (-1.0)).abs() < f32::EPSILON);
}

// ---------------------------------------------------------------------------
// MAD tests
// ---------------------------------------------------------------------------

#[test]
fn test_median_and_mad_odd() {
    let mut values = [2.0f32, 4.0, 3.0];
    let (median, mad) = median_and_mad_f32_mut(&mut values);
    assert!((median - 3.0).abs() < 1e-6);
    assert!((mad - 1.0).abs() < 1e-6);
}

#[test]
fn test_median_and_mad_uniform() {
    let mut values = [3.5f32, 3.5, 3.5, 3.5, 3.5];
    let (median, mad) = median_and_mad_f32_mut(&mut values);
    assert!((median - 3.5).abs() < 1e-6);
    assert!(mad.abs() < 1e-6);
}

#[test]
fn test_mad_with_scratch() {
    let values = [2.0f32, 4.0, 3.0];
    let mut scratch = Vec::new();
    let mad = mad_f32_with_scratch(&values, 3.0, &mut scratch);
    assert!((mad - 1.0).abs() < 1e-6);
}

#[test]
fn test_mad_with_scratch_empty() {
    let values: [f32; 0] = [];
    let mut scratch = Vec::new();
    let mad = mad_f32_with_scratch(&values, 0.0, &mut scratch);
    assert!(mad.abs() < f32::EPSILON);
}

// ---------------------------------------------------------------------------
// Sigma-clipped median/MAD tests
// ---------------------------------------------------------------------------

#[test]
fn test_sigma_clipped_empty_input() {
    let mut values: Vec<f32> = vec![];
    let mut deviations = Vec::new();
    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
    assert_eq!(median, 0.0);
    assert_eq!(sigma, 0.0);
}

#[test]
fn test_sigma_clipped_single_value() {
    let mut values = vec![5.0];
    let mut deviations = Vec::new();
    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
    assert_eq!(median, 5.0);
    assert_eq!(sigma, 0.0);
}

#[test]
fn test_sigma_clipped_two_values() {
    let mut values = vec![2.0, 4.0];
    let mut deviations = Vec::new();
    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
    assert!((median - 3.0).abs() < 0.01);
}

#[test]
fn test_sigma_clipped_uniform_values() {
    let mut values = vec![5.0; 100];
    let mut deviations = Vec::new();
    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
    assert_eq!(median, 5.0);
    assert_eq!(sigma, 0.0);
}

#[test]
fn test_sigma_clipped_no_outliers() {
    let mut values: Vec<f32> = (0..100).map(|i| 50.0 + (i as f32 - 50.0) * 0.1).collect();
    let mut deviations = Vec::new();
    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
    assert!((median - 50.0).abs() < 1.0);
    assert!(sigma > 0.0 && sigma < 10.0);
}

#[test]
fn test_sigma_clipped_rejects_outliers() {
    let mut values: Vec<f32> = vec![10.0; 97];
    values.extend([1000.0, 2000.0, 3000.0]);
    let original_len = values.len();
    let mut deviations = Vec::new();

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    assert!((median - 10.0).abs() < 0.1);
    assert!(sigma < 1.0);
    assert_eq!(values.len(), original_len);
}

#[test]
fn test_sigma_clipped_negative_values() {
    let mut values = vec![-10.0, -5.0, 0.0, 5.0, 10.0];
    let mut deviations = Vec::new();
    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
    assert!((median - 0.0).abs() < 0.1);
    assert!(sigma > 0.0);
}

#[test]
fn test_sigma_clipped_mixed_outliers() {
    let mut values: Vec<f32> = vec![100.0; 90];
    values.extend([0.0, 1.0, 2.0, 198.0, 199.0, 200.0]);
    values.extend([99.0, 100.0, 101.0, 102.0]);
    let mut deviations = Vec::new();

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
    assert!((median - 100.0).abs() < 2.0);
}

#[test]
fn test_sigma_clipped_zero_iterations() {
    let mut values = vec![1.0, 2.0, 3.0, 1000.0];
    let mut deviations = Vec::new();
    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 0);
    assert!((median - 2.5).abs() < 0.1);
}

#[test]
fn test_sigma_clipped_one_iteration() {
    let mut values: Vec<f32> = vec![10.0; 10];
    values.push(10000.0);
    let mut deviations = Vec::new();

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 1);

    assert!((median - 10.0).abs() < 0.1);
    assert!(sigma < 1.0);
}

#[test]
fn test_sigma_clipped_kappa_affects_clipping() {
    let base_values: Vec<f32> = {
        let mut v = vec![50.0; 90];
        v.extend([20.0, 25.0, 75.0, 80.0]);
        v.extend([0.0, 100.0]);
        v
    };

    let mut values_strict = base_values.clone();
    let mut values_loose = base_values.clone();
    let mut deviations = Vec::new();

    let (median_strict, sigma_strict) =
        sigma_clipped_median_mad(&mut values_strict, &mut deviations, 1.5, 3);
    let (median_loose, sigma_loose) =
        sigma_clipped_median_mad(&mut values_loose, &mut deviations, 5.0, 3);

    assert!((median_strict - 50.0).abs() < 5.0);
    assert!((median_loose - 50.0).abs() < 5.0);
    assert!(sigma_strict <= sigma_loose);
}

#[test]
fn test_sigma_clipped_deviations_buffer_reused() {
    let mut values1 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let mut values2 = vec![10.0, 20.0, 30.0];
    let mut deviations = Vec::new();

    sigma_clipped_median_mad(&mut values1, &mut deviations, 3.0, 2);
    let cap_after_first = deviations.capacity();

    sigma_clipped_median_mad(&mut values2, &mut deviations, 3.0, 2);

    assert!(deviations.capacity() >= cap_after_first.min(values2.len()));
}

#[test]
fn test_sigma_clipped_large_dataset() {
    let mut values: Vec<f32> = (0..10000).map(|i| 100.0 + (i % 10) as f32).collect();
    for i in 0..100 {
        values[i * 100] = 1000.0;
    }
    let mut deviations = Vec::new();

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    assert!((100.0..=110.0).contains(&median));
    assert!(sigma > 0.0 && sigma < 20.0);
}

#[test]
fn test_sigma_clipped_all_same_then_one_different() {
    let mut values: Vec<f32> = vec![42.0; 999];
    values.push(9999.0);
    let mut deviations = Vec::new();

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    assert!((median - 42.0).abs() < 0.01);
    assert!(sigma < 0.01);
}

// ---------------------------------------------------------------------------
// NaN handling tests
// ---------------------------------------------------------------------------

#[test]
fn test_median_with_nan_does_not_panic() {
    let mut values = [1.0f32, f32::NAN, 3.0, 2.0, 5.0];
    // Should not panic — NaN sorts to end via total_cmp
    let median = median_f32_mut(&mut values);
    assert!(!median.is_nan());
}

#[test]
fn test_sigma_clip_with_nan_does_not_panic() {
    let mut values = vec![10.0f32; 20];
    values[5] = f32::NAN;
    values[15] = f32::NAN;
    let mut deviations = Vec::new();
    // Should not panic
    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
    assert!(!median.is_nan());
}

// ---------------------------------------------------------------------------
// Sigma clip index correspondence regression test
// ---------------------------------------------------------------------------

#[test]
fn test_sigma_clip_asymmetric_outliers() {
    // Regression test for the index mismatch bug where select_nth_unstable_by
    // on the deviations buffer broke the correspondence with the values buffer.
    // With asymmetric outliers, the bug would clip wrong values.
    let mut values: Vec<f32> = vec![100.0; 50];
    // Add outliers only on the high side
    values.extend([500.0, 600.0, 700.0, 800.0, 900.0]);
    let mut deviations = Vec::new();

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 2.5, 5);

    assert!(
        (median - 100.0).abs() < 1.0,
        "median should be ~100, got {}",
        median
    );
    assert!(
        sigma < 5.0,
        "sigma should be small after clipping outliers, got {}",
        sigma
    );
}

// ---------------------------------------------------------------------------
// abs_deviation_inplace tests
// ---------------------------------------------------------------------------

#[test]
fn test_abs_deviation_inplace_basic() {
    let mut values = [1.0f32, 2.0, 3.0, 4.0, 5.0];
    abs_deviation_inplace(&mut values, 3.0);
    assert_eq!(values, [2.0, 1.0, 0.0, 1.0, 2.0]);
}

#[test]
fn test_abs_deviation_inplace_negative() {
    let mut values = [-4.0f32, -2.0, 0.0, 2.0, 4.0];
    abs_deviation_inplace(&mut values, 0.0);
    assert_eq!(values, [4.0, 2.0, 0.0, 2.0, 4.0]);
}

#[test]
fn test_abs_deviation_inplace_single() {
    let mut values = [5.0f32];
    abs_deviation_inplace(&mut values, 3.0);
    assert_eq!(values, [2.0]);
}

#[test]
fn test_abs_deviation_inplace_empty() {
    let mut values: [f32; 0] = [];
    abs_deviation_inplace(&mut values, 0.0);
    assert!(values.is_empty());
}

// ---------------------------------------------------------------------------
// MAD to sigma conversion tests
// ---------------------------------------------------------------------------

#[test]
fn test_mad_to_sigma_known_value() {
    let sigma = mad_to_sigma(1.0);
    assert!((sigma - MAD_TO_SIGMA).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// mad_f32_with_scratch edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_mad_with_scratch_single() {
    let values = [5.0f32];
    let mut scratch = Vec::new();
    let mad = mad_f32_with_scratch(&values, 5.0, &mut scratch);
    assert!(mad.abs() < f32::EPSILON);
}

#[test]
fn test_mad_with_scratch_two_elements() {
    let values = [2.0f32, 8.0];
    let mut scratch = Vec::new();
    // median of [2, 8] = 5, deviations = [3, 3], MAD = 3
    let mad = mad_f32_with_scratch(&values, 5.0, &mut scratch);
    assert!((mad - 3.0).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// sigma_clipped_median_mad_arrayvec tests
// ---------------------------------------------------------------------------

#[test]
fn test_sigma_clipped_arrayvec_basic() {
    let mut values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];
    let mut deviations: arrayvec::ArrayVec<f32, 16> = arrayvec::ArrayVec::new();
    let (median, sigma) = sigma_clipped_median_mad_arrayvec(&mut values, &mut deviations, 3.0, 3);
    assert!((median - 3.0).abs() < 0.1);
    assert!(sigma > 0.0);
}

#[test]
fn test_sigma_clipped_arrayvec_empty() {
    let mut values: Vec<f32> = vec![];
    let mut deviations: arrayvec::ArrayVec<f32, 16> = arrayvec::ArrayVec::new();
    let (median, sigma) = sigma_clipped_median_mad_arrayvec(&mut values, &mut deviations, 3.0, 3);
    assert_eq!(median, 0.0);
    assert_eq!(sigma, 0.0);
}

#[test]
fn test_sigma_clipped_arrayvec_single() {
    let mut values = vec![42.0f32];
    let mut deviations: arrayvec::ArrayVec<f32, 16> = arrayvec::ArrayVec::new();
    let (median, sigma) = sigma_clipped_median_mad_arrayvec(&mut values, &mut deviations, 3.0, 3);
    assert_eq!(median, 42.0);
    assert_eq!(sigma, 0.0);
}

#[test]
fn test_sigma_clipped_arrayvec_rejects_outliers() {
    let mut values: Vec<f32> = vec![10.0; 12];
    values.extend([1000.0, 2000.0]);
    let mut deviations: arrayvec::ArrayVec<f32, 16> = arrayvec::ArrayVec::new();

    let (median, sigma) = sigma_clipped_median_mad_arrayvec(&mut values, &mut deviations, 3.0, 3);

    assert!((median - 10.0).abs() < 0.1);
    assert!(sigma < 1.0);
}

#[test]
fn test_sigma_clipped_arrayvec_uniform() {
    let mut values = vec![5.0f32; 10];
    let mut deviations: arrayvec::ArrayVec<f32, 16> = arrayvec::ArrayVec::new();
    let (median, sigma) = sigma_clipped_median_mad_arrayvec(&mut values, &mut deviations, 3.0, 3);
    assert_eq!(median, 5.0);
    assert_eq!(sigma, 0.0);
}

#[test]
fn test_sigma_clipped_arrayvec_matches_vec_version() {
    let base_values: Vec<f32> = vec![1.0, 2.0, 3.0, 100.0, 4.0, 5.0, 6.0, 200.0];

    let mut values_vec = base_values.clone();
    let mut deviations_vec = Vec::new();
    let (median_vec, sigma_vec) =
        sigma_clipped_median_mad(&mut values_vec, &mut deviations_vec, 3.0, 3);

    let mut values_arrayvec = base_values.clone();
    let mut deviations_arrayvec: arrayvec::ArrayVec<f32, 16> = arrayvec::ArrayVec::new();
    let (median_arrayvec, sigma_arrayvec) =
        sigma_clipped_median_mad_arrayvec(&mut values_arrayvec, &mut deviations_arrayvec, 3.0, 3);

    assert!((median_vec - median_arrayvec).abs() < 1e-6);
    assert!((sigma_vec - sigma_arrayvec).abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// median_f32_fast tests
// ---------------------------------------------------------------------------

#[test]
fn test_median_f32_fast_odd() {
    // Sorted: [1, 2, 3, 5, 8], mid=2, median=3
    let mut values = [5.0f32, 2.0, 8.0, 1.0, 3.0];
    assert!((median_f32_fast(&mut values) - 3.0).abs() < f32::EPSILON);
}

#[test]
fn test_median_f32_fast_even_returns_upper_middle() {
    // Sorted: [1, 2, 5, 8], mid=2, returns values[2]=5 (upper-middle)
    // This differs from median_f32_mut which returns (2+5)/2 = 3.5
    let mut values = [5.0f32, 2.0, 8.0, 1.0];
    let fast = median_f32_fast(&mut values);
    assert!(
        (fast - 5.0).abs() < f32::EPSILON,
        "expected 5.0, got {fast}"
    );
}

#[test]
fn test_median_f32_fast_differs_from_exact_on_even() {
    // Sorted: [1, 3, 7, 9], mid=2
    // Exact: (3+7)/2 = 5.0
    // Fast: values[2] = 7.0
    let mut values_fast = [9.0f32, 1.0, 7.0, 3.0];
    let mut values_exact = values_fast;
    let fast = median_f32_fast(&mut values_fast);
    let exact = median_f32_mut(&mut values_exact);
    assert!((exact - 5.0).abs() < f32::EPSILON);
    assert!((fast - 7.0).abs() < f32::EPSILON);
    assert!(
        (fast - exact).abs() > 1.0,
        "fast and exact should differ for even N"
    );
}

#[test]
fn test_median_f32_fast_agrees_with_exact_on_odd() {
    // For odd N, both return the same middle element
    // Sorted: [2, 4, 6, 8, 10], mid=2, median=6
    let mut values_fast = [10.0f32, 4.0, 6.0, 2.0, 8.0];
    let mut values_exact = values_fast;
    let fast = median_f32_fast(&mut values_fast);
    let exact = median_f32_mut(&mut values_exact);
    assert!((fast - exact).abs() < f32::EPSILON);
    assert!((fast - 6.0).abs() < f32::EPSILON);
}

#[test]
fn test_median_f32_fast_single() {
    let mut values = [42.0f32];
    assert!((median_f32_fast(&mut values) - 42.0).abs() < f32::EPSILON);
}

#[test]
fn test_median_f32_fast_two_elements() {
    // Sorted: [3, 7], mid=1, returns 7 (upper-middle)
    let mut values = [7.0f32, 3.0];
    assert!((median_f32_fast(&mut values) - 7.0).abs() < f32::EPSILON);
}

#[test]
fn test_median_f32_fast_all_equal() {
    let mut values = [5.0f32; 20];
    assert!((median_f32_fast(&mut values) - 5.0).abs() < f32::EPSILON);
}

#[test]
fn test_median_f32_fast_negative_values() {
    // Sorted: [-10, -5, -2, 3, 7], mid=2, median=-2
    let mut values = [3.0f32, -5.0, 7.0, -10.0, -2.0];
    assert!((median_f32_fast(&mut values) - (-2.0)).abs() < f32::EPSILON);
}

// ---------------------------------------------------------------------------
// mad_f32_fast tests
// ---------------------------------------------------------------------------

#[test]
fn test_mad_f32_fast_hand_computed() {
    // values = [2, 3, 4], median = 3
    // deviations = |2-3|, |3-3|, |4-3| = [1, 0, 1]
    // sorted deviations: [0, 1, 1], mid=1, MAD = 1
    let values = [2.0f32, 3.0, 4.0];
    let mut scratch = Vec::new();
    let mad = mad_f32_fast(&values, 3.0, &mut scratch);
    assert!((mad - 1.0).abs() < f32::EPSILON);
}

#[test]
fn test_mad_f32_fast_five_values() {
    // values = [1, 2, 3, 4, 5], median = 3
    // deviations = [2, 1, 0, 1, 2]
    // sorted deviations: [0, 1, 1, 2, 2], mid=2, MAD = 1
    let values = [1.0f32, 2.0, 3.0, 4.0, 5.0];
    let mut scratch = Vec::new();
    let mad = mad_f32_fast(&values, 3.0, &mut scratch);
    assert!((mad - 1.0).abs() < f32::EPSILON);
}

#[test]
fn test_mad_f32_fast_uniform() {
    // All same → all deviations = 0 → MAD = 0
    let values = [7.0f32; 10];
    let mut scratch = Vec::new();
    let mad = mad_f32_fast(&values, 7.0, &mut scratch);
    assert!(mad.abs() < f32::EPSILON);
}

#[test]
fn test_mad_f32_fast_empty() {
    let values: [f32; 0] = [];
    let mut scratch = Vec::new();
    let mad = mad_f32_fast(&values, 0.0, &mut scratch);
    assert!(mad.abs() < f32::EPSILON);
}

#[test]
fn test_mad_f32_fast_single() {
    // Single value: deviation = 0, MAD = 0
    let values = [5.0f32];
    let mut scratch = Vec::new();
    let mad = mad_f32_fast(&values, 5.0, &mut scratch);
    assert!(mad.abs() < f32::EPSILON);
}

#[test]
fn test_mad_f32_fast_scratch_reused() {
    // Verify scratch buffer is reused (capacity preserved across calls)
    let mut scratch = Vec::new();

    let values1 = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
    mad_f32_fast(&values1, 5.5, &mut scratch);
    let cap = scratch.capacity();
    assert!(cap >= 10);

    let values2 = [1.0f32, 2.0, 3.0];
    mad_f32_fast(&values2, 2.0, &mut scratch);
    assert!(scratch.capacity() >= cap, "capacity should not shrink");
}

#[test]
fn test_mad_f32_fast_matches_regular_on_odd() {
    // For odd N, median_f32_fast and median_f32_mut agree,
    // so mad_f32_fast should match mad_f32_with_scratch exactly.
    let values = [10.0f32, 2.0, 7.0, 15.0, 3.0];
    let median = 7.0; // sorted: [2, 3, 7, 10, 15], mid=2
    let mut scratch1 = Vec::new();
    let mut scratch2 = Vec::new();
    let mad_fast = mad_f32_fast(&values, median, &mut scratch1);
    let mad_regular = mad_f32_with_scratch(&values, median, &mut scratch2);
    // deviations = |10-7|, |2-7|, |7-7|, |15-7|, |3-7| = [3, 5, 0, 8, 4]
    // sorted = [0, 3, 4, 5, 8], mid=2 → MAD = 4
    assert!(
        (mad_fast - mad_regular).abs() < f32::EPSILON,
        "fast={mad_fast}, regular={mad_regular}"
    );
    assert!((mad_fast - 4.0).abs() < f32::EPSILON);
}

// ---------------------------------------------------------------------------
// sigma_clipped_median_mad tests (moved from background/tests.rs)
// ---------------------------------------------------------------------------

#[test]
fn test_sigma_clipped_stats_empty_values() {
    let mut values: Vec<f32> = vec![];
    let mut deviations: Vec<f32> = vec![];

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    assert!((median - 0.0).abs() < 1e-6);
    assert!((sigma - 0.0).abs() < 1e-6);
}

#[test]
fn test_sigma_clipped_stats_single_value() {
    let mut values = vec![0.5];
    let mut deviations: Vec<f32> = vec![];

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    assert!((median - 0.5).abs() < 1e-6);
    assert!((sigma - 0.0).abs() < 1e-6);
}

#[test]
fn test_sigma_clipped_stats_uniform_values() {
    let mut values = vec![0.3; 100];
    let mut deviations: Vec<f32> = vec![];

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    assert!((median - 0.3).abs() < 1e-6);
    assert!((sigma - 0.0).abs() < 1e-6);
}

#[test]
fn test_sigma_clipped_stats_no_outliers() {
    // Normal-ish distribution without outliers
    let mut values: Vec<f32> = (0..100).map(|i| 0.5 + (i as f32 - 50.0) * 0.001).collect();
    let mut deviations: Vec<f32> = vec![];

    let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.5
    assert!(
        (median - 0.5).abs() < 0.01,
        "Median {} should be ~0.5",
        median
    );
    // Sigma should be small but non-zero
    assert!(sigma > 0.0, "Sigma should be positive");
    assert!(sigma < 0.1, "Sigma {} should be small", sigma);
}

#[test]
fn test_sigma_clipped_stats_rejects_high_outliers() {
    // 90 values at 0.2, 10 high outliers at 0.9
    let mut values: Vec<f32> = vec![0.2; 90];
    values.extend(vec![0.9; 10]);
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.2 (outliers rejected)
    assert!(
        (median - 0.2).abs() < 0.05,
        "Median {} should be ~0.2 after rejecting outliers",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_rejects_low_outliers() {
    // 90 values at 0.8, 10 low outliers at 0.1
    let mut values: Vec<f32> = vec![0.8; 90];
    values.extend(vec![0.1; 10]);
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.8 (outliers rejected)
    assert!(
        (median - 0.8).abs() < 0.05,
        "Median {} should be ~0.8 after rejecting outliers",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_rejects_both_tails() {
    // 80 values at 0.5, 10 low outliers, 10 high outliers
    let mut values: Vec<f32> = vec![0.5; 80];
    values.extend(vec![0.05; 10]); // Low outliers
    values.extend(vec![0.95; 10]); // High outliers
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~0.5 (both tails rejected)
    assert!(
        (median - 0.5).abs() < 0.05,
        "Median {} should be ~0.5 after rejecting outliers",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_kappa_affects_rejection() {
    // Good values: 50 at 0.50, 30 at 0.54 (true center = 0.50)
    // Outliers: 20 at 0.80
    //
    // Approx median of all 100 = 0.54 (upper-middle, value[50]).
    // MAD = 0.04 (deviations: 50×0.04, 30×0.00, 20×0.26).
    // sigma = 0.04 * 1.4826 = 0.059.
    //
    // kappa=1.5: threshold = 0.089. Rejects 0.80 (dev 0.26 > 0.089).
    //   After clipping: 80 values [0.50(50), 0.54(30)].
    //   Iter 2: approx median = 0.50, MAD = 0 → converge at 0.50.
    //
    // kappa=5.0: threshold = 0.297. Keeps 0.80 (dev 0.26 < 0.297).
    //   Converge at approx median = 0.54.
    let base_values: Vec<f32> = {
        let mut v = vec![0.50; 50];
        v.extend(vec![0.54; 30]);
        v.extend(vec![0.80; 20]);
        v
    };

    let mut values_strict = base_values.clone();
    let mut values_loose = base_values.clone();
    let mut deviations: Vec<f32> = vec![];

    let (median_strict, _) = sigma_clipped_median_mad(&mut values_strict, &mut deviations, 1.5, 3);
    deviations.clear();
    let (median_loose, _) = sigma_clipped_median_mad(&mut values_loose, &mut deviations, 5.0, 3);

    // Strict rejects outliers → converges at 0.50 (true center)
    // Loose keeps outliers → converges at 0.54 (biased)
    assert!(
        (median_strict - 0.5).abs() < (median_loose - 0.5).abs(),
        "Strict kappa median {} should be closer to 0.5 than loose {}",
        median_strict,
        median_loose
    );
    assert!(
        (median_strict - 0.5).abs() < 1e-6,
        "Strict kappa should recover true median 0.5, got {}",
        median_strict
    );
}

#[test]
fn test_sigma_clipped_stats_iterations_improve_result() {
    // Good values: 41 at 0.30, 40 at 0.32 (true median = 0.30, odd count = 81)
    // Outliers: 10 at 0.60, 9 at 1.50
    //
    // Approx median of all 100 = 0.32 (value[50]).
    // MAD = 0.02 (devs: 41×0.02, 40×0.00, 10×0.28, 9×1.18, index 50 = 0.02).
    // sigma = 0.02 * 1.4826 = 0.0297.
    //
    // 0 iterations (no clipping): compute_final_stats on 100 values.
    //   median_f32_mut(100): avg(values[50], max(values[0..50])) = avg(0.32, 0.32) = 0.32.
    //
    // 3 iterations (with clipping):
    //   Iter 1: kappa=2.5, threshold = 0.074. Rejects 0.60 and 1.50 → 81 remain.
    //   Iter 2: 81 values (odd). approx median = value[40] = 0.30.
    //     MAD = 0.00, sigma = 0 → converge at 0.30.
    let base_values: Vec<f32> = {
        let mut v = vec![0.30; 41];
        v.extend(vec![0.32; 40]);
        v.extend(vec![0.60; 10]);
        v.extend(vec![1.50; 9]);
        v
    };

    let mut values_0iter = base_values.clone();
    let mut values_3iter = base_values.clone();
    let mut deviations: Vec<f32> = vec![];

    let (median_0iter, _) = sigma_clipped_median_mad(&mut values_0iter, &mut deviations, 2.5, 0);
    deviations.clear();
    let (median_3iter, _) = sigma_clipped_median_mad(&mut values_3iter, &mut deviations, 2.5, 3);

    // 0 iterations: no clipping, median biased to 0.32 by outlier presence
    assert!(
        (median_0iter - 0.32).abs() < 1e-6,
        "0 iterations should give 0.32, got {}",
        median_0iter
    );
    // 3 iterations: clipping removes outliers, converges to true median 0.30
    assert!(
        (median_3iter - 0.30).abs() < 1e-6,
        "3 iterations should recover true median 0.30, got {}",
        median_3iter
    );
    // Clipping brings result closer to true center
    assert!(
        (median_3iter - 0.30).abs() < (median_0iter - 0.30).abs(),
        "3 iterations median {} should be closer to 0.30 than 0 iterations {}",
        median_3iter,
        median_0iter
    );
}

#[test]
fn test_sigma_clipped_stats_mad_to_sigma_conversion() {
    // MAD * 1.4826 ≈ sigma for Gaussian distribution
    // Create data with known spread
    let mut values: Vec<f32> = (-50..=50).map(|i| 0.5 + i as f32 * 0.002).collect();
    let mut deviations: Vec<f32> = vec![];

    let (_median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 10.0, 1); // High kappa = no clipping

    // Data: 101 evenly spaced values from 0.4 to 0.6 (step = 0.002)
    // Median = 0.5 (center value)
    // |x - 0.5| values: 0.000, 0.002, ..., 0.100 (101 values, each appearing once)
    // Sorted abs deviations: [0.000, 0.002, 0.002, 0.004, 0.004, ..., 0.100]
    // MAD = median of abs devs = value at index 50 of 101 sorted abs devs
    // Abs devs sorted: each deviation d=0.000..0.100 in steps of 0.002 appears twice
    // (positive and negative), except 0.000 which appears once.
    // So sorted: [0.000, 0.002, 0.002, 0.004, 0.004, ..., 0.100, 0.100]
    // Index 50 → 0.050
    // sigma = MAD * 1.4826 = 0.050 * 1.4826 = 0.07413
    let expected_sigma = 0.05 * 1.4826;
    assert!(
        (sigma - expected_sigma).abs() < 0.002,
        "Sigma {} should be ~{:.4} (MAD=0.05 × 1.4826)",
        sigma,
        expected_sigma
    );
}

#[test]
fn test_sigma_clipped_stats_preserves_deviations_buffer() {
    let mut values = vec![0.1, 0.2, 0.3, 0.4, 0.5];
    let mut deviations: Vec<f32> = Vec::with_capacity(100);

    sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Buffer should be reused (capacity preserved)
    assert!(
        deviations.capacity() >= 5,
        "Deviations buffer should have been used"
    );
}

#[test]
fn test_sigma_clipped_stats_handles_two_values() {
    let mut values = vec![0.3, 0.7];
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // With only 2 values, iteration stops (len < 3) and final stats are computed.
    // median_f32_mut on 2 values (even length): averages two middle elements
    // = (0.3 + 0.7) / 2 = 0.5
    assert!(
        (median - 0.5).abs() < 1e-6,
        "Median of [0.3, 0.7] should be 0.5 (average of two), got {}",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_zero_iterations() {
    let mut values = vec![0.2, 0.2, 0.2, 0.9, 0.9];
    let mut deviations: Vec<f32> = vec![];

    // Zero iterations = just compute stats without clipping
    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 0);

    // Median of [0.2, 0.2, 0.2, 0.9, 0.9] sorted = [0.2, 0.2, 0.2, 0.9, 0.9] -> median = 0.2
    assert!(
        (median - 0.2).abs() < 1e-6,
        "Median {} should be 0.2",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_extreme_outlier() {
    // Single extreme outlier among many normal values
    let mut values: Vec<f32> = vec![0.5; 99];
    values.push(100.0); // Extreme outlier
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Outlier should be rejected, median should be 0.5
    assert!(
        (median - 0.5).abs() < 0.01,
        "Median {} should be ~0.5",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_negative_values() {
    let mut values: Vec<f32> = vec![-0.5; 90];
    values.extend(vec![0.5; 10]); // Outliers on positive side
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be ~-0.5
    assert!(
        (median - (-0.5)).abs() < 0.05,
        "Median {} should be ~-0.5",
        median
    );
}

#[test]
fn test_sigma_clipped_stats_all_same_except_one() {
    // Edge case: all values same except one outlier
    let mut values: Vec<f32> = vec![0.4; 99];
    values.push(0.9);
    let mut deviations: Vec<f32> = vec![];

    let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

    // Median should be 0.4, sigma should be 0 or near-zero after clipping
    assert!(
        (median - 0.4).abs() < 1e-6,
        "Median {} should be 0.4",
        median
    );
}
