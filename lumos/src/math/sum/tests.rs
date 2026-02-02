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
// sum_squared_diff tests
// ---------------------------------------------------------------------------

#[test]
fn test_sum_squared_diff() {
    let values: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let mean_val: f32 = 4.5;
    let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
    assert!((sum_squared_diff(&values, mean_val) - expected).abs() < 1e-4);
}

#[test]
fn test_sum_squared_diff_remainder() {
    let values: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
    let mean_val: f32 = 4.0;
    let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
    assert!((sum_squared_diff(&values, mean_val) - expected).abs() < 1e-4);
}

#[test]
fn test_sum_squared_diff_small() {
    let values: Vec<f32> = vec![1.0, 2.0, 3.0];
    let mean_val: f32 = 2.0;
    let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
    assert!((sum_squared_diff(&values, mean_val) - expected).abs() < 1e-4);
}

#[test]
fn test_sum_squared_diff_negative() {
    let values: Vec<f32> = vec![-4.0, -2.0, 0.0, 2.0, 4.0, 6.0, 8.0, 10.0];
    let mean_val: f32 = 3.0;
    let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
    assert!((sum_squared_diff(&values, mean_val) - expected).abs() < 1e-4);
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
// accumulate tests
// ---------------------------------------------------------------------------

#[test]
fn test_accumulate() {
    let mut dst: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let src: Vec<f32> = vec![0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5];
    accumulate(&mut dst, &src);
    assert_eq!(dst, vec![1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5]);
}

#[test]
fn test_accumulate_remainder() {
    let mut dst: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let src: Vec<f32> = vec![0.5, 0.5, 0.5, 0.5, 0.5];
    accumulate(&mut dst, &src);
    assert_eq!(dst, vec![1.5, 2.5, 3.5, 4.5, 5.5]);
}

#[test]
fn test_accumulate_small() {
    let mut dst: Vec<f32> = vec![1.0, 2.0];
    let src: Vec<f32> = vec![0.5, 0.5];
    accumulate(&mut dst, &src);
    assert_eq!(dst, vec![1.5, 2.5]);
}

// ---------------------------------------------------------------------------
// scale tests
// ---------------------------------------------------------------------------

#[test]
fn test_scale() {
    let mut data: Vec<f32> = vec![2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0];
    scale(&mut data, 0.5);
    assert_eq!(data, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
}

#[test]
fn test_scale_remainder() {
    let mut data: Vec<f32> = vec![2.0, 4.0, 6.0, 8.0, 10.0];
    scale(&mut data, 0.5);
    assert_eq!(data, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
}

#[test]
fn test_scale_small() {
    let mut data: Vec<f32> = vec![2.0, 4.0];
    scale(&mut data, 0.5);
    assert_eq!(data, vec![1.0, 2.0]);
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

#[test]
fn test_simd_vs_scalar_sum_squared_diff() {
    let values: Vec<f32> = (0..1000).map(|x| x as f32 * 0.1).collect();
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let scalar_result = scalar::sum_squared_diff(&values, mean);
    let simd_result = sum_squared_diff(&values, mean);
    assert!(
        (scalar_result - simd_result).abs() < 1e-1,
        "scalar={}, simd={}",
        scalar_result,
        simd_result
    );
}

#[test]
fn test_simd_vs_scalar_accumulate() {
    let mut dst_scalar: Vec<f32> = (0..1000).map(|x| x as f32).collect();
    let mut dst_simd: Vec<f32> = dst_scalar.clone();
    let src: Vec<f32> = (0..1000).map(|x| x as f32 * 0.1).collect();

    scalar::accumulate(&mut dst_scalar, &src);
    accumulate(&mut dst_simd, &src);

    for (s, d) in dst_scalar.iter().zip(dst_simd.iter()) {
        assert!((s - d).abs() < 1e-4, "scalar={}, simd={}", s, d);
    }
}

#[test]
fn test_simd_vs_scalar_scale() {
    let mut data_scalar: Vec<f32> = (0..1000).map(|x| x as f32).collect();
    let mut data_simd: Vec<f32> = data_scalar.clone();

    scalar::scale(&mut data_scalar, 0.123);
    scale(&mut data_simd, 0.123);

    for (s, d) in data_scalar.iter().zip(data_simd.iter()) {
        assert!((s - d).abs() < 1e-4, "scalar={}, simd={}", s, d);
    }
}
