//! Tests for deviation operations.

use super::*;

#[test]
fn test_abs_deviation_inplace_basic() {
    let mut values = [1.0f32, 2.0, 3.0, 4.0, 5.0];
    let median = 3.0;
    abs_deviation_inplace(&mut values, median);
    assert_eq!(values, [2.0, 1.0, 0.0, 1.0, 2.0]);
}

#[test]
fn test_abs_deviation_inplace_negative() {
    let mut values = [-4.0f32, -2.0, 0.0, 2.0, 4.0];
    let median = 0.0;
    abs_deviation_inplace(&mut values, median);
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

#[test]
fn test_simd_vs_scalar_abs_deviation() {
    let original: Vec<f32> = (0..1000).map(|x| x as f32 * 0.1 - 50.0).collect();
    let median = 0.0;

    let mut scalar_data = original.clone();
    let mut simd_data = original.clone();

    scalar::abs_deviation_inplace(&mut scalar_data, median);
    abs_deviation_inplace(&mut simd_data, median);

    for (s, d) in scalar_data.iter().zip(simd_data.iter()) {
        assert!((s - d).abs() < 1e-6, "scalar={}, simd={}", s, d);
    }
}

#[test]
fn test_abs_deviation_various_sizes() {
    for size in [1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 100] {
        let original: Vec<f32> = (0..size).map(|x| x as f32).collect();
        let median = size as f32 / 2.0;

        let mut scalar_data = original.clone();
        let mut simd_data = original.clone();

        scalar::abs_deviation_inplace(&mut scalar_data, median);
        abs_deviation_inplace(&mut simd_data, median);

        for (i, (s, d)) in scalar_data.iter().zip(simd_data.iter()).enumerate() {
            assert!(
                (s - d).abs() < 1e-6,
                "size={}, i={}, scalar={}, simd={}",
                size,
                i,
                s,
                d
            );
        }
    }
}
