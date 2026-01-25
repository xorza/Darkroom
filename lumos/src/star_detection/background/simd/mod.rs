//! SIMD-accelerated background estimation utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

// These functions are prepared for future integration but not yet used in hot paths
#![allow(dead_code)]

#[cfg(target_arch = "x86_64")]
use crate::common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

/// Compute sum and sum of squares for a slice of f32 values using SIMD.
///
/// Returns (sum, sum_of_squares).
#[inline]
pub fn sum_and_sum_sq_simd(values: &[f32]) -> (f32, f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2() {
            return unsafe { sse::sum_and_sum_sq_avx2(values) };
        }
        if cpu_features::has_sse4_1() {
            return unsafe { sse::sum_and_sum_sq_sse41(values) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { neon::sum_and_sum_sq_neon(values) };
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    sum_and_sum_sq_scalar(values)
}

/// Compute the sum of absolute deviations from a median using SIMD.
///
/// Returns the sum of |value - median| for all values.
#[inline]
pub fn sum_abs_deviations_simd(values: &[f32], median: f32) -> f32 {
    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2() {
            return unsafe { sse::sum_abs_deviations_avx2(values, median) };
        }
        if cpu_features::has_sse4_1() {
            return unsafe { sse::sum_abs_deviations_sse41(values, median) };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { neon::sum_abs_deviations_neon(values, median) };
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    sum_abs_deviations_scalar(values, median)
}

/// Scalar implementation of sum and sum of squares.
#[inline]
fn sum_and_sum_sq_scalar(values: &[f32]) -> (f32, f32) {
    let mut sum = 0.0f32;
    let mut sum_sq = 0.0f32;

    for &v in values {
        sum += v;
        sum_sq += v * v;
    }

    (sum, sum_sq)
}

/// Scalar implementation of sum of absolute deviations.
#[inline]
fn sum_abs_deviations_scalar(values: &[f32], median: f32) -> f32 {
    values.iter().map(|&v| (v - median).abs()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sum_and_sum_sq_scalar() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let (sum, sum_sq) = sum_and_sum_sq_scalar(&values);

        assert!((sum - 15.0).abs() < 1e-6);
        assert!((sum_sq - 55.0).abs() < 1e-6); // 1 + 4 + 9 + 16 + 25
    }

    #[test]
    fn test_sum_abs_deviations_scalar() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let median = 3.0;
        let sum_dev = sum_abs_deviations_scalar(&values, median);

        // |1-3| + |2-3| + |3-3| + |4-3| + |5-3| = 2 + 1 + 0 + 1 + 2 = 6
        assert!((sum_dev - 6.0).abs() < 1e-6);
    }

    #[test]
    fn test_sum_and_sum_sq_simd_matches_scalar() {
        let values: Vec<f32> = (0..1000).map(|i| i as f32 * 0.1).collect();

        let (sum_simd, sum_sq_simd) = sum_and_sum_sq_simd(&values);
        let (sum_scalar, sum_sq_scalar) = sum_and_sum_sq_scalar(&values);

        assert!(
            (sum_simd - sum_scalar).abs() < 0.1,
            "Sum mismatch: {} vs {}",
            sum_simd,
            sum_scalar
        );
        assert!(
            (sum_sq_simd - sum_sq_scalar).abs() < 1.0,
            "Sum sq mismatch: {} vs {}",
            sum_sq_simd,
            sum_sq_scalar
        );
    }

    #[test]
    fn test_sum_abs_deviations_simd_matches_scalar() {
        let values: Vec<f32> = (0..1000).map(|i| i as f32 * 0.1).collect();
        let median = 50.0;

        let sum_simd = sum_abs_deviations_simd(&values, median);
        let sum_scalar = sum_abs_deviations_scalar(&values, median);

        assert!(
            (sum_simd - sum_scalar).abs() < 0.1,
            "Sum abs dev mismatch: {} vs {}",
            sum_simd,
            sum_scalar
        );
    }

    // ========== Comprehensive SIMD vs Scalar tests ==========

    #[test]
    fn test_sum_and_sum_sq_simd_empty_array() {
        let values: Vec<f32> = vec![];
        let (sum_simd, sum_sq_simd) = sum_and_sum_sq_simd(&values);
        let (sum_scalar, sum_sq_scalar) = sum_and_sum_sq_scalar(&values);

        assert_eq!(sum_simd, sum_scalar);
        assert_eq!(sum_sq_simd, sum_sq_scalar);
    }

    #[test]
    fn test_sum_and_sum_sq_simd_single_value() {
        let values = vec![42.5];
        let (sum_simd, sum_sq_simd) = sum_and_sum_sq_simd(&values);
        let (sum_scalar, sum_sq_scalar) = sum_and_sum_sq_scalar(&values);

        assert!((sum_simd - sum_scalar).abs() < 1e-6);
        assert!((sum_sq_simd - sum_sq_scalar).abs() < 1e-6);
    }

    #[test]
    fn test_sum_and_sum_sq_simd_small_array_less_than_simd_width() {
        // Test arrays smaller than SIMD register width (4 for SSE, 8 for AVX)
        for size in 1..16 {
            let values: Vec<f32> = (0..size).map(|i| (i as f32) * 1.5 + 0.7).collect();
            let (sum_simd, sum_sq_simd) = sum_and_sum_sq_simd(&values);
            let (sum_scalar, sum_sq_scalar) = sum_and_sum_sq_scalar(&values);

            assert!(
                (sum_simd - sum_scalar).abs() < 1e-4,
                "Size {}: sum mismatch {} vs {}",
                size,
                sum_simd,
                sum_scalar
            );
            assert!(
                (sum_sq_simd - sum_sq_scalar).abs() < 1e-3,
                "Size {}: sum_sq mismatch {} vs {}",
                size,
                sum_sq_simd,
                sum_sq_scalar
            );
        }
    }

    #[test]
    fn test_sum_and_sum_sq_simd_all_same_values() {
        let values = vec![7.5; 100];
        let (sum_simd, sum_sq_simd) = sum_and_sum_sq_simd(&values);
        let (sum_scalar, sum_sq_scalar) = sum_and_sum_sq_scalar(&values);

        assert!(
            (sum_simd - sum_scalar).abs() < 1e-4,
            "Sum mismatch: {} vs {}",
            sum_simd,
            sum_scalar
        );
        assert!(
            (sum_sq_simd - sum_sq_scalar).abs() < 1e-3,
            "Sum sq mismatch: {} vs {}",
            sum_sq_simd,
            sum_sq_scalar
        );
    }

    #[test]
    fn test_sum_and_sum_sq_simd_negative_values() {
        let values: Vec<f32> = (-50..50).map(|i| i as f32 * 0.1).collect();
        let (sum_simd, sum_sq_simd) = sum_and_sum_sq_simd(&values);
        let (sum_scalar, sum_sq_scalar) = sum_and_sum_sq_scalar(&values);

        assert!(
            (sum_simd - sum_scalar).abs() < 1e-4,
            "Sum mismatch: {} vs {}",
            sum_simd,
            sum_scalar
        );
        assert!(
            (sum_sq_simd - sum_sq_scalar).abs() < 1e-2,
            "Sum sq mismatch: {} vs {}",
            sum_sq_simd,
            sum_sq_scalar
        );
    }

    #[test]
    fn test_sum_and_sum_sq_simd_large_values() {
        let values: Vec<f32> = (0..100).map(|i| (i as f32) * 1000.0 + 50000.0).collect();
        let (sum_simd, sum_sq_simd) = sum_and_sum_sq_simd(&values);
        let (sum_scalar, sum_sq_scalar) = sum_and_sum_sq_scalar(&values);

        let rel_err_sum = (sum_simd - sum_scalar).abs() / sum_scalar.abs();
        let rel_err_sq = (sum_sq_simd - sum_sq_scalar).abs() / sum_sq_scalar.abs();

        assert!(
            rel_err_sum < 1e-5,
            "Sum relative error too large: {}",
            rel_err_sum
        );
        assert!(
            rel_err_sq < 1e-4,
            "Sum sq relative error too large: {}",
            rel_err_sq
        );
    }

    #[test]
    fn test_sum_and_sum_sq_simd_random_pattern() {
        // Pseudo-random pattern using sin
        let values: Vec<f32> = (0..500)
            .map(|i| (i as f32 * 0.7).sin() * 100.0 + 50.0)
            .collect();
        let (sum_simd, sum_sq_simd) = sum_and_sum_sq_simd(&values);
        let (sum_scalar, sum_sq_scalar) = sum_and_sum_sq_scalar(&values);

        assert!(
            (sum_simd - sum_scalar).abs() < 0.5,
            "Sum mismatch: {} vs {}",
            sum_simd,
            sum_scalar
        );
        assert!(
            (sum_sq_simd - sum_sq_scalar).abs() < 50.0,
            "Sum sq mismatch: {} vs {}",
            sum_sq_simd,
            sum_sq_scalar
        );
    }

    #[test]
    fn test_sum_abs_deviations_simd_empty_array() {
        let values: Vec<f32> = vec![];
        let sum_simd = sum_abs_deviations_simd(&values, 0.0);
        let sum_scalar = sum_abs_deviations_scalar(&values, 0.0);

        assert_eq!(sum_simd, sum_scalar);
    }

    #[test]
    fn test_sum_abs_deviations_simd_single_value() {
        let values = vec![42.5];
        let median = 40.0;
        let sum_simd = sum_abs_deviations_simd(&values, median);
        let sum_scalar = sum_abs_deviations_scalar(&values, median);

        assert!((sum_simd - sum_scalar).abs() < 1e-6);
    }

    #[test]
    fn test_sum_abs_deviations_simd_small_array_less_than_simd_width() {
        for size in 1..16 {
            let values: Vec<f32> = (0..size).map(|i| (i as f32) * 2.0 + 1.0).collect();
            let median = size as f32; // roughly middle
            let sum_simd = sum_abs_deviations_simd(&values, median);
            let sum_scalar = sum_abs_deviations_scalar(&values, median);

            assert!(
                (sum_simd - sum_scalar).abs() < 1e-4,
                "Size {}: mismatch {} vs {}",
                size,
                sum_simd,
                sum_scalar
            );
        }
    }

    #[test]
    fn test_sum_abs_deviations_simd_all_equal_to_median() {
        let values = vec![5.0; 100];
        let median = 5.0;
        let sum_simd = sum_abs_deviations_simd(&values, median);
        let sum_scalar = sum_abs_deviations_scalar(&values, median);

        assert!(
            (sum_simd - sum_scalar).abs() < 1e-6,
            "Mismatch: {} vs {}",
            sum_simd,
            sum_scalar
        );
        assert!(sum_simd < 1e-6, "Should be nearly zero: {}", sum_simd);
    }

    #[test]
    fn test_sum_abs_deviations_simd_negative_median() {
        let values: Vec<f32> = (-50..50).map(|i| i as f32).collect();
        let median = -10.0;
        let sum_simd = sum_abs_deviations_simd(&values, median);
        let sum_scalar = sum_abs_deviations_scalar(&values, median);

        assert!(
            (sum_simd - sum_scalar).abs() < 1e-4,
            "Mismatch: {} vs {}",
            sum_simd,
            sum_scalar
        );
    }

    #[test]
    fn test_sum_abs_deviations_simd_random_pattern() {
        let values: Vec<f32> = (0..500)
            .map(|i| (i as f32 * 0.3).cos() * 50.0 + 25.0)
            .collect();
        let median = 25.0;
        let sum_simd = sum_abs_deviations_simd(&values, median);
        let sum_scalar = sum_abs_deviations_scalar(&values, median);

        assert!(
            (sum_simd - sum_scalar).abs() < 0.5,
            "Mismatch: {} vs {}",
            sum_simd,
            sum_scalar
        );
    }

    #[test]
    fn test_sum_abs_deviations_simd_extreme_outliers() {
        let mut values: Vec<f32> = vec![100.0; 100];
        values[0] = 0.0; // extreme low
        values[99] = 10000.0; // extreme high
        let median = 100.0;

        let sum_simd = sum_abs_deviations_simd(&values, median);
        let sum_scalar = sum_abs_deviations_scalar(&values, median);

        assert!(
            (sum_simd - sum_scalar).abs() < 1e-2,
            "Mismatch: {} vs {}",
            sum_simd,
            sum_scalar
        );
    }
}
