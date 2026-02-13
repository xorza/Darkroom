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
        (scalar_result - simd_result).abs() < 1e-2,
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
