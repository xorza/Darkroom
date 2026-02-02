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
// MAD to sigma conversion tests
// ---------------------------------------------------------------------------

#[test]
fn test_mad_to_sigma_known_value() {
    let sigma = mad_to_sigma(1.0);
    assert!((sigma - MAD_TO_SIGMA).abs() < 1e-6);
}
