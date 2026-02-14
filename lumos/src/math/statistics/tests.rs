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
