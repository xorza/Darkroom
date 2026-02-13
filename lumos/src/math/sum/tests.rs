//! Tests for sum operations.

use super::*;

// ---------------------------------------------------------------------------
// sum_f32 tests
// ---------------------------------------------------------------------------

#[test]
fn test_sum_f32() {
    let values: Vec<f32> = (1..=16).map(|x| x as f32).collect();
    let expected: f32 = values.iter().sum();
    assert!((sum_f32(&values) - expected).abs() < 1e-4);
}

#[test]
fn test_sum_f32_remainder() {
    let values: Vec<f32> = (1..=13).map(|x| x as f32).collect();
    let expected: f32 = values.iter().sum();
    assert!((sum_f32(&values) - expected).abs() < 1e-4);
}

#[test]
fn test_sum_f32_small() {
    let values = vec![1.0f32, 2.0, 3.0];
    let expected: f32 = values.iter().sum();
    assert!((sum_f32(&values) - expected).abs() < 1e-4);
}

#[test]
fn test_sum_f32_single() {
    assert!((sum_f32(&[42.0]) - 42.0).abs() < f32::EPSILON);
}

#[test]
fn test_sum_f32_empty() {
    assert!((sum_f32(&[]) - 0.0).abs() < f32::EPSILON);
}

#[test]
fn test_sum_f32_negative() {
    let values: Vec<f32> = vec![-1.0, -2.0, -3.0, -4.0, 5.0, 6.0, 7.0, 8.0];
    let expected: f32 = values.iter().sum();
    assert!((sum_f32(&values) - expected).abs() < 1e-4);
}

// ---------------------------------------------------------------------------
// mean_f32 tests
// ---------------------------------------------------------------------------

#[test]
fn test_mean_f32() {
    let values: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    assert!((mean_f32(&values) - 4.5).abs() < 1e-4);
}

#[test]
fn test_mean_f32_single() {
    assert!((mean_f32(&[42.0]) - 42.0).abs() < f32::EPSILON);
}

// ---------------------------------------------------------------------------
// SIMD vs scalar consistency tests
// ---------------------------------------------------------------------------

#[test]
fn test_simd_vs_scalar_sum() {
    let values: Vec<f32> = (0..1000).map(|x| x as f32 * 0.1).collect();
    let scalar_result = scalar::sum_f32(&values);
    let simd_result = sum_f32(&values);
    assert!(
        (scalar_result - simd_result).abs() < 1e-5,
        "scalar={}, simd={}",
        scalar_result,
        simd_result
    );
}

// ---------------------------------------------------------------------------
// SIMD boundary tests (exact chunk sizes for SSE/AVX2)
// ---------------------------------------------------------------------------

#[test]
fn test_sum_f32_simd_boundaries() {
    // Test exact SIMD chunk boundaries: 4 (SSE), 8 (AVX2), 16, 32
    for size in [4, 8, 16, 32] {
        let values: Vec<f32> = (1..=size).map(|x| x as f32).collect();
        let expected: f32 = values.iter().sum();
        let result = sum_f32(&values);
        assert!(
            (result - expected).abs() < 1e-4,
            "size={}, expected={}, got={}",
            size,
            expected,
            result
        );
    }
}

#[test]
fn test_sum_f32_simd_boundary_minus_one() {
    // Test one less than chunk boundaries (remainder handling)
    for size in [3, 7, 15, 31] {
        let values: Vec<f32> = (1..=size).map(|x| x as f32).collect();
        let expected: f32 = values.iter().sum();
        let result = sum_f32(&values);
        assert!(
            (result - expected).abs() < 1e-4,
            "size={}, expected={}, got={}",
            size,
            expected,
            result
        );
    }
}

#[test]
fn test_sum_f32_simd_boundary_plus_one() {
    // Test one more than chunk boundaries
    for size in [5, 9, 17, 33] {
        let values: Vec<f32> = (1..=size).map(|x| x as f32).collect();
        let expected: f32 = values.iter().sum();
        let result = sum_f32(&values);
        assert!(
            (result - expected).abs() < 1e-4,
            "size={}, expected={}, got={}",
            size,
            expected,
            result
        );
    }
}

#[test]
fn test_mean_f32_two_elements() {
    let values = [3.0f32, 7.0];
    assert!((mean_f32(&values) - 5.0).abs() < f32::EPSILON);
}

// ---------------------------------------------------------------------------
// weighted_mean_f32 tests
// ---------------------------------------------------------------------------

#[test]
fn test_weighted_mean_uniform_weights() {
    let values = [1.0f32, 2.0, 3.0, 4.0, 5.0];
    let weights = [1.0f32; 5];
    let result = weighted_mean_f32(&values, &weights);
    assert!((result - 3.0).abs() < 1e-6);
}

#[test]
fn test_weighted_mean_varying_weights() {
    // Weighted mean of [10, 20] with weights [3, 1] = (30 + 20) / 4 = 12.5
    let values = [10.0f32, 20.0];
    let weights = [3.0f32, 1.0];
    let result = weighted_mean_f32(&values, &weights);
    assert!((result - 12.5).abs() < 1e-6);
}

#[test]
fn test_weighted_mean_single_value() {
    let result = weighted_mean_f32(&[42.0], &[5.0]);
    assert!((result - 42.0).abs() < 1e-6);
}

#[test]
fn test_weighted_mean_empty() {
    let result = weighted_mean_f32(&[], &[]);
    assert_eq!(result, 0.0);
}

#[test]
fn test_weighted_mean_zero_weights_returns_zero() {
    let result = weighted_mean_f32(&[1.0, 2.0, 3.0], &[0.0, 0.0, 0.0]);
    assert_eq!(result, 0.0);
}

#[test]
fn test_weighted_mean_one_nonzero_weight() {
    // Only the value with nonzero weight should matter
    let values = [10.0f32, 20.0, 30.0];
    let weights = [0.0f32, 5.0, 0.0];
    let result = weighted_mean_f32(&values, &weights);
    assert!((result - 20.0).abs() < 1e-6);
}

#[test]
fn test_weighted_mean_negative_values() {
    let values = [-10.0f32, 10.0];
    let weights = [1.0f32, 1.0];
    let result = weighted_mean_f32(&values, &weights);
    assert!(result.abs() < 1e-6);
}

// ---------------------------------------------------------------------------
// Precision tests for compensated summation
// ---------------------------------------------------------------------------

#[test]
fn test_sum_f32_precision_large_constant() {
    // Sum 100k values of 1.0: exact answer is 100000.0
    // Naive f32 summation would accumulate rounding errors; Neumaier should be exact.
    let n = 100_000;
    let values = vec![1.0f32; n];
    let result = sum_f32(&values);
    assert_eq!(result, n as f32, "sum of {} ones should be exact", n);
}

#[test]
fn test_sum_f32_precision_catastrophic_cancellation() {
    // Big positive + big negative + small values: naive summation loses the small values.
    // Neumaier should preserve them.
    let big = 1e7f32;
    let small = 1e-3f32;
    let n_small = 1000;
    let mut values = vec![big, -big];
    values.extend(std::iter::repeat_n(small, n_small));

    let expected = n_small as f32 * small; // 1.0 exactly
    let result = sum_f32(&values);
    assert!(
        (result - expected).abs() < 1e-4,
        "expected {expected}, got {result} — compensation failed"
    );
}

#[test]
fn test_sum_f32_precision_scalar_vs_f64() {
    // Compare scalar Neumaier result against f64 reference for typical astronomy data.
    let values: Vec<f32> = (0..4096).map(|i| 100.0 + (i as f32) * 0.01).collect();
    let f64_sum: f64 = values.iter().map(|&v| v as f64).sum();

    let result = scalar::sum_f32(&values);
    let error = (result as f64 - f64_sum).abs();
    assert!(
        error < 0.1,
        "scalar Neumaier error {error:.6} too large (f64 ref: {f64_sum:.6}, got: {result:.6})"
    );
}

#[test]
fn test_sum_f32_precision_simd_vs_f64() {
    // Compare SIMD Neumaier result against f64 reference.
    let values: Vec<f32> = (0..4096).map(|i| 100.0 + (i as f32) * 0.01).collect();
    let f64_sum: f64 = values.iter().map(|&v| v as f64).sum();

    let result = sum_f32(&values);
    let error = (result as f64 - f64_sum).abs();
    assert!(
        error < 0.1,
        "SIMD Neumaier error {error:.6} too large (f64 ref: {f64_sum:.6}, got: {result:.6})"
    );
}

#[test]
fn test_weighted_mean_precision_large_array() {
    // Weighted mean of uniform values with uniform weights should equal the value.
    let n = 10_000;
    let values = vec![42.5f32; n];
    let weights = vec![1.0f32; n];
    let result = weighted_mean_f32(&values, &weights);
    assert!(
        (result - 42.5).abs() < 1e-5,
        "weighted mean of {n} × 42.5 should be 42.5, got {result}"
    );
}

#[test]
fn test_sum_f32_compensation_actually_helps() {
    // Construct data where naive summation provably loses precision.
    // Sum of 1e6 + (1e-1 repeated 10000 times) + (-1e6).
    // Naive: 1e6 + 0.1 = 1e6 (lost!), repeated. Final: 1e6 - 1e6 = 0.
    // Compensated: should recover ~1000.0.
    let mut values = vec![1e6f32];
    values.extend(std::iter::repeat_n(0.1f32, 10_000));
    values.push(-1e6f32);

    let expected = 10_000.0 * 0.1; // 1000.0
    let result = sum_f32(&values);
    assert!(
        (result - expected as f32).abs() < 1.0,
        "compensation should recover small values: expected ~{expected}, got {result}"
    );

    // Verify scalar path independently
    let scalar_result = scalar::sum_f32(&values);
    assert!(
        (scalar_result - expected as f32).abs() < 1.0,
        "scalar compensation should recover small values: expected ~{expected}, got {scalar_result}"
    );
}

#[test]
fn test_sum_f32_simd_vs_scalar_catastrophic() {
    // Both paths should handle catastrophic cancellation similarly.
    let big = 1e7f32;
    let small = 1e-3f32;
    let n_small = 1000;
    let mut values = vec![big, -big];
    values.extend(std::iter::repeat_n(small, n_small));

    let scalar_result = scalar::sum_f32(&values);
    let simd_result = sum_f32(&values);
    let expected = 1.0f32;

    assert!(
        (scalar_result - expected).abs() < 1e-3,
        "scalar: expected ~{expected}, got {scalar_result}"
    );
    assert!(
        (simd_result - expected).abs() < 1e-3,
        "simd: expected ~{expected}, got {simd_result}"
    );
}

#[test]
fn test_sum_f32_simd_vs_scalar_large_f64_ref() {
    // Both paths should agree closely on large realistic data.
    let values: Vec<f32> = (0..10_000).map(|i| (i as f32 + 1.0) * 0.7).collect();
    let f64_ref: f64 = values.iter().map(|&v| v as f64).sum();

    let scalar_result = scalar::sum_f32(&values);
    let simd_result = sum_f32(&values);

    let scalar_err = (scalar_result as f64 - f64_ref).abs();
    let simd_err = (simd_result as f64 - f64_ref).abs();

    assert!(
        scalar_err < 1.0,
        "scalar error {scalar_err:.6} vs f64 ref {f64_ref:.2}"
    );
    assert!(
        simd_err < 1.0,
        "simd error {simd_err:.6} vs f64 ref {f64_ref:.2}"
    );
    // Both should be close to each other
    assert!(
        (scalar_result - simd_result).abs() < 1.0,
        "scalar={scalar_result}, simd={simd_result}"
    );
}

#[test]
fn test_sum_f32_all_negative() {
    let values: Vec<f32> = (1..=4096).map(|i| -(i as f32) * 0.1).collect();
    let f64_ref: f64 = values.iter().map(|&v| v as f64).sum();

    let result = sum_f32(&values);
    let error = (result as f64 - f64_ref).abs();
    assert!(
        error < 0.5,
        "all-negative sum error {error:.6} (f64 ref: {f64_ref:.2}, got: {result})"
    );
}

#[test]
fn test_weighted_mean_compensation_helps() {
    // Large number of values near a mean — naive summation of v*w accumulates error.
    let n = 50_000;
    let values: Vec<f32> = (0..n).map(|i| 1000.0 + (i as f32) * 0.001).collect();
    let weights = vec![1.0f32; n];

    let f64_mean: f64 = values.iter().map(|&v| v as f64).sum::<f64>() / n as f64;

    let result = weighted_mean_f32(&values, &weights);
    let error = (result as f64 - f64_mean).abs();
    assert!(
        error < 0.01,
        "weighted mean error {error:.6} (f64 ref: {f64_mean:.6}, got: {result})"
    );
}
