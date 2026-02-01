//! NEON SIMD implementations of math operations (aarch64).

use std::arch::aarch64::*;

/// Sum f32 values using NEON SIMD.
///
/// # Safety
/// Caller must ensure this runs on aarch64 with NEON support.
#[target_feature(enable = "neon")]
pub unsafe fn sum_f32(values: &[f32]) -> f32 {
    unsafe {
        let mut sum_vec = vdupq_n_f32(0.0);
        let chunks = values.chunks_exact(4);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let v = vld1q_f32(chunk.as_ptr());
            sum_vec = vaddq_f32(sum_vec, v);
        }

        let sum = vaddvq_f32(sum_vec);
        sum + remainder.iter().sum::<f32>()
    }
}

/// Calculate sum of squared differences from mean using NEON SIMD.
///
/// # Safety
/// Caller must ensure this runs on aarch64 with NEON support.
#[target_feature(enable = "neon")]
pub unsafe fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    unsafe {
        let mean_vec = vdupq_n_f32(mean);
        let mut sum_vec = vdupq_n_f32(0.0);
        let chunks = values.chunks_exact(4);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let v = vld1q_f32(chunk.as_ptr());
            let diff = vsubq_f32(v, mean_vec);
            let sq = vmulq_f32(diff, diff);
            sum_vec = vaddq_f32(sum_vec, sq);
        }

        let sum = vaddvq_f32(sum_vec);
        sum + remainder.iter().map(|v| (v - mean).powi(2)).sum::<f32>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::scalar;

    // NEON is always available on aarch64

    // -------------------------------------------------------------------------
    // sum_f32 tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_sum_exact_multiple_of_4() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let expected = scalar::sum_f32(&values);
        let result = unsafe { sum_f32(&values) };
        assert!((result - expected).abs() < 1e-5);
    }

    #[test]
    fn test_sum_with_remainder_1() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0]; // 4 + 1
        let expected = scalar::sum_f32(&values);
        let result = unsafe { sum_f32(&values) };
        assert!((result - expected).abs() < 1e-5);
    }

    #[test]
    fn test_sum_with_remainder_2() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]; // 4 + 2
        let expected = scalar::sum_f32(&values);
        let result = unsafe { sum_f32(&values) };
        assert!((result - expected).abs() < 1e-5);
    }

    #[test]
    fn test_sum_with_remainder_3() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]; // 4 + 3
        let expected = scalar::sum_f32(&values);
        let result = unsafe { sum_f32(&values) };
        assert!((result - expected).abs() < 1e-5);
    }

    #[test]
    fn test_sum_less_than_4() {
        let values = [1.0, 2.0, 3.0];
        let expected = scalar::sum_f32(&values);
        let result = unsafe { sum_f32(&values) };
        assert!((result - expected).abs() < 1e-5);
    }

    #[test]
    fn test_sum_empty() {
        let values: [f32; 0] = [];
        let result = unsafe { sum_f32(&values) };
        assert!((result - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_negative_values() {
        let values = [-1.0, -2.0, -3.0, -4.0, 5.0, 6.0, 7.0, 8.0];
        let expected = scalar::sum_f32(&values);
        let result = unsafe { sum_f32(&values) };
        assert!((result - expected).abs() < 1e-5);
    }

    #[test]
    fn test_sum_large_values() {
        let values = [1e6, 2e6, 3e6, 4e6, 5e6, 6e6, 7e6, 8e6];
        let expected = scalar::sum_f32(&values);
        let result = unsafe { sum_f32(&values) };
        assert!((result - expected).abs() / expected < 1e-5);
    }

    #[test]
    fn test_sum_small_values() {
        let values = [1e-6, 2e-6, 3e-6, 4e-6, 5e-6, 6e-6, 7e-6, 8e-6];
        let expected = scalar::sum_f32(&values);
        let result = unsafe { sum_f32(&values) };
        assert!((result - expected).abs() < 1e-10);
    }

    #[test]
    fn test_sum_large_array() {
        let values: Vec<f32> = (1..=10000).map(|x| x as f32 * 0.01).collect();
        let expected = scalar::sum_f32(&values);
        let result = unsafe { sum_f32(&values) };
        assert!((result - expected).abs() / expected < 1e-4);
    }

    // -------------------------------------------------------------------------
    // sum_squared_diff tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_sum_squared_diff_exact_multiple_of_4() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let mean = 4.5;
        let expected = scalar::sum_squared_diff(&values, mean);
        let result = unsafe { sum_squared_diff(&values, mean) };
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff_with_remainder() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]; // 4 + 3
        let mean = 4.0;
        let expected = scalar::sum_squared_diff(&values, mean);
        let result = unsafe { sum_squared_diff(&values, mean) };
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff_less_than_4() {
        let values = [1.0, 2.0, 3.0];
        let mean = 2.0;
        let expected = scalar::sum_squared_diff(&values, mean);
        let result = unsafe { sum_squared_diff(&values, mean) };
        assert!((result - expected).abs() < 1e-5);
    }

    #[test]
    fn test_sum_squared_diff_empty() {
        let values: [f32; 0] = [];
        let result = unsafe { sum_squared_diff(&values, 0.0) };
        assert!((result - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_squared_diff_all_equal_to_mean() {
        let values = [5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0];
        let result = unsafe { sum_squared_diff(&values, 5.0) };
        assert!((result - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_squared_diff_negative_values() {
        let values = [-4.0, -2.0, 0.0, 2.0, 4.0, 6.0, 8.0, 10.0];
        let mean = 3.0;
        let expected = scalar::sum_squared_diff(&values, mean);
        let result = unsafe { sum_squared_diff(&values, mean) };
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff_large_array() {
        let values: Vec<f32> = (0..10000).map(|x| x as f32 * 0.1).collect();
        let mean = 499.95; // approx mean
        let expected = scalar::sum_squared_diff(&values, mean);
        let result = unsafe { sum_squared_diff(&values, mean) };
        assert!((result - expected).abs() / expected < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff_zero_mean() {
        let values = [-2.0, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        let expected = scalar::sum_squared_diff(&values, 0.0);
        let result = unsafe { sum_squared_diff(&values, 0.0) };
        assert!((result - expected).abs() < 1e-4);
    }
}
