//! SIMD-accelerated background estimation utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

// These functions are prepared for future integration but not yet used in hot paths
#![allow(dead_code)]

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
        if is_x86_feature_detected!("avx2") {
            return unsafe { sse::sum_and_sum_sq_avx2(values) };
        }
        if is_x86_feature_detected!("sse4.1") {
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
        if is_x86_feature_detected!("avx2") {
            return unsafe { sse::sum_abs_deviations_avx2(values, median) };
        }
        if is_x86_feature_detected!("sse4.1") {
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
}
