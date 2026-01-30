//! SIMD-accelerated background estimation utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

/// Compute sum and sum of squares for a slice of f32 values using SIMD.
///
/// Returns (sum, sum_of_squares).
#[allow(dead_code)] // Kept for future use and testing
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
#[allow(dead_code)] // Kept for future use and testing
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
#[allow(dead_code)] // Used in tests
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
#[allow(dead_code)] // Used in tests
#[inline]
fn sum_abs_deviations_scalar(values: &[f32], median: f32) -> f32 {
    values.iter().map(|&v| (v - median).abs()).sum()
}

/// Bilinear interpolation for a row segment using SIMD.
///
/// Fills bg_out and noise_out with interpolated values between left and right endpoints.
/// The interpolation weight starts at `wx_start` at position 0 and increases by `wx_step`
/// for each subsequent pixel.
///
/// # Arguments
/// * `bg_out` - Output slice for background values
/// * `noise_out` - Output slice for noise values
/// * `left_bg` - Background value at left edge
/// * `right_bg` - Background value at right edge
/// * `left_noise` - Noise value at left edge
/// * `right_noise` - Noise value at right edge
/// * `wx_start` - Starting interpolation weight (0.0 at left edge)
/// * `wx_step` - Weight increment per pixel
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn interpolate_segment_simd(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    left_bg: f32,
    right_bg: f32,
    left_noise: f32,
    right_noise: f32,
    wx_start: f32,
    wx_step: f32,
) {
    debug_assert_eq!(bg_out.len(), noise_out.len());

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            unsafe {
                interpolate_segment_avx2(
                    bg_out,
                    noise_out,
                    left_bg,
                    right_bg,
                    left_noise,
                    right_noise,
                    wx_start,
                    wx_step,
                );
            }
            return;
        }
        if cpu_features::has_sse4_1() {
            unsafe {
                interpolate_segment_sse(
                    bg_out,
                    noise_out,
                    left_bg,
                    right_bg,
                    left_noise,
                    right_noise,
                    wx_start,
                    wx_step,
                );
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            interpolate_segment_neon(
                bg_out,
                noise_out,
                left_bg,
                right_bg,
                left_noise,
                right_noise,
                wx_start,
                wx_step,
            );
        }
        return;
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    interpolate_segment_scalar(
        bg_out,
        noise_out,
        left_bg,
        right_bg,
        left_noise,
        right_noise,
        wx_start,
        wx_step,
    );
}

/// Scalar implementation of segment interpolation.
#[allow(clippy::too_many_arguments)]
#[inline]
fn interpolate_segment_scalar(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    left_bg: f32,
    right_bg: f32,
    left_noise: f32,
    right_noise: f32,
    wx_start: f32,
    wx_step: f32,
) {
    let delta_bg = right_bg - left_bg;
    let delta_noise = right_noise - left_noise;

    for (i, (bg, noise)) in bg_out.iter_mut().zip(noise_out.iter_mut()).enumerate() {
        let wx = (wx_start + i as f32 * wx_step).clamp(0.0, 1.0);
        *bg = left_bg + wx * delta_bg;
        *noise = left_noise + wx * delta_noise;
    }
}

#[cfg(target_arch = "x86_64")]
#[allow(clippy::too_many_arguments)]
#[target_feature(enable = "avx2,fma")]
unsafe fn interpolate_segment_avx2(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    left_bg: f32,
    right_bg: f32,
    left_noise: f32,
    right_noise: f32,
    wx_start: f32,
    wx_step: f32,
) {
    use std::arch::x86_64::*;

    let len = bg_out.len();
    let delta_bg = right_bg - left_bg;
    let delta_noise = right_noise - left_noise;

    unsafe {
        let left_bg_v = _mm256_set1_ps(left_bg);
        let delta_bg_v = _mm256_set1_ps(delta_bg);
        let left_noise_v = _mm256_set1_ps(left_noise);
        let delta_noise_v = _mm256_set1_ps(delta_noise);
        let wx_step_8 = _mm256_set1_ps(wx_step * 8.0);
        let zero = _mm256_setzero_ps();
        let one = _mm256_set1_ps(1.0);

        // Initial weights: wx_start, wx_start+step, wx_start+2*step, ...
        let mut wx_v = _mm256_set_ps(
            wx_start + 7.0 * wx_step,
            wx_start + 6.0 * wx_step,
            wx_start + 5.0 * wx_step,
            wx_start + 4.0 * wx_step,
            wx_start + 3.0 * wx_step,
            wx_start + 2.0 * wx_step,
            wx_start + wx_step,
            wx_start,
        );

        let mut i = 0;
        while i + 8 <= len {
            // Clamp weights to [0, 1]
            let wx_clamped = _mm256_min_ps(_mm256_max_ps(wx_v, zero), one);

            // bg = left_bg + wx * delta_bg
            let bg_v = _mm256_fmadd_ps(wx_clamped, delta_bg_v, left_bg_v);
            // noise = left_noise + wx * delta_noise
            let noise_v = _mm256_fmadd_ps(wx_clamped, delta_noise_v, left_noise_v);

            _mm256_storeu_ps(bg_out.as_mut_ptr().add(i), bg_v);
            _mm256_storeu_ps(noise_out.as_mut_ptr().add(i), noise_v);

            wx_v = _mm256_add_ps(wx_v, wx_step_8);
            i += 8;
        }

        // Handle remainder with SSE (4 at a time)
        if i + 4 <= len {
            let left_bg_v4 = _mm_set1_ps(left_bg);
            let delta_bg_v4 = _mm_set1_ps(delta_bg);
            let left_noise_v4 = _mm_set1_ps(left_noise);
            let delta_noise_v4 = _mm_set1_ps(delta_noise);
            let zero4 = _mm_setzero_ps();
            let one4 = _mm_set1_ps(1.0);

            let current_wx = wx_start + i as f32 * wx_step;
            let wx_v4 = _mm_set_ps(
                current_wx + 3.0 * wx_step,
                current_wx + 2.0 * wx_step,
                current_wx + wx_step,
                current_wx,
            );

            let wx_clamped = _mm_min_ps(_mm_max_ps(wx_v4, zero4), one4);
            let bg_v4 = _mm_fmadd_ps(wx_clamped, delta_bg_v4, left_bg_v4);
            let noise_v4 = _mm_fmadd_ps(wx_clamped, delta_noise_v4, left_noise_v4);

            _mm_storeu_ps(bg_out.as_mut_ptr().add(i), bg_v4);
            _mm_storeu_ps(noise_out.as_mut_ptr().add(i), noise_v4);
            i += 4;
        }

        // Handle final remainder scalar
        while i < len {
            let wx = (wx_start + i as f32 * wx_step).clamp(0.0, 1.0);
            bg_out[i] = left_bg + wx * delta_bg;
            noise_out[i] = left_noise + wx * delta_noise;
            i += 1;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[allow(clippy::too_many_arguments)]
#[target_feature(enable = "sse4.1")]
unsafe fn interpolate_segment_sse(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    left_bg: f32,
    right_bg: f32,
    left_noise: f32,
    right_noise: f32,
    wx_start: f32,
    wx_step: f32,
) {
    use std::arch::x86_64::*;

    let len = bg_out.len();
    let delta_bg = right_bg - left_bg;
    let delta_noise = right_noise - left_noise;

    unsafe {
        let left_bg_v = _mm_set1_ps(left_bg);
        let delta_bg_v = _mm_set1_ps(delta_bg);
        let left_noise_v = _mm_set1_ps(left_noise);
        let delta_noise_v = _mm_set1_ps(delta_noise);
        let wx_step_4 = _mm_set1_ps(wx_step * 4.0);
        let zero = _mm_setzero_ps();
        let one = _mm_set1_ps(1.0);

        // Initial weights
        let mut wx_v = _mm_set_ps(
            wx_start + 3.0 * wx_step,
            wx_start + 2.0 * wx_step,
            wx_start + wx_step,
            wx_start,
        );

        let mut i = 0;
        while i + 4 <= len {
            // Clamp weights to [0, 1]
            let wx_clamped = _mm_min_ps(_mm_max_ps(wx_v, zero), one);

            // bg = left_bg + wx * delta_bg (using mul + add since SSE4.1 doesn't have FMA)
            let bg_v = _mm_add_ps(left_bg_v, _mm_mul_ps(wx_clamped, delta_bg_v));
            let noise_v = _mm_add_ps(left_noise_v, _mm_mul_ps(wx_clamped, delta_noise_v));

            _mm_storeu_ps(bg_out.as_mut_ptr().add(i), bg_v);
            _mm_storeu_ps(noise_out.as_mut_ptr().add(i), noise_v);

            wx_v = _mm_add_ps(wx_v, wx_step_4);
            i += 4;
        }

        // Handle remainder scalar
        while i < len {
            let wx = (wx_start + i as f32 * wx_step).clamp(0.0, 1.0);
            bg_out[i] = left_bg + wx * delta_bg;
            noise_out[i] = left_noise + wx * delta_noise;
            i += 1;
        }
    }
}

#[cfg(target_arch = "aarch64")]
#[allow(clippy::too_many_arguments)]
unsafe fn interpolate_segment_neon(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    left_bg: f32,
    right_bg: f32,
    left_noise: f32,
    right_noise: f32,
    wx_start: f32,
    wx_step: f32,
) {
    use std::arch::aarch64::*;

    let len = bg_out.len();
    let delta_bg = right_bg - left_bg;
    let delta_noise = right_noise - left_noise;

    unsafe {
        let left_bg_v = vdupq_n_f32(left_bg);
        let delta_bg_v = vdupq_n_f32(delta_bg);
        let left_noise_v = vdupq_n_f32(left_noise);
        let delta_noise_v = vdupq_n_f32(delta_noise);
        let wx_step_4 = vdupq_n_f32(wx_step * 4.0);
        let zero = vdupq_n_f32(0.0);
        let one = vdupq_n_f32(1.0);

        // Initial weights: [wx_start, wx_start+step, wx_start+2*step, wx_start+3*step]
        let initial_offsets: [f32; 4] = [0.0, wx_step, 2.0 * wx_step, 3.0 * wx_step];
        let mut wx_v = vaddq_f32(vdupq_n_f32(wx_start), vld1q_f32(initial_offsets.as_ptr()));

        let mut i = 0;
        while i + 4 <= len {
            // Clamp weights to [0, 1]
            let wx_clamped = vminq_f32(vmaxq_f32(wx_v, zero), one);

            // bg = left_bg + wx * delta_bg (FMA)
            let bg_v = vfmaq_f32(left_bg_v, wx_clamped, delta_bg_v);
            let noise_v = vfmaq_f32(left_noise_v, wx_clamped, delta_noise_v);

            vst1q_f32(bg_out.as_mut_ptr().add(i), bg_v);
            vst1q_f32(noise_out.as_mut_ptr().add(i), noise_v);

            wx_v = vaddq_f32(wx_v, wx_step_4);
            i += 4;
        }

        // Handle remainder scalar
        while i < len {
            let wx = (wx_start + i as f32 * wx_step).clamp(0.0, 1.0);
            bg_out[i] = left_bg + wx * delta_bg;
            noise_out[i] = left_noise + wx * delta_noise;
            i += 1;
        }
    }
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

    // ========== Interpolation SIMD tests ==========

    #[test]
    fn test_interpolate_segment_simd_matches_scalar() {
        for len in [1, 3, 4, 7, 8, 15, 16, 31, 64, 100] {
            let mut bg_simd = vec![0.0f32; len];
            let mut noise_simd = vec![0.0f32; len];
            let mut bg_scalar = vec![0.0f32; len];
            let mut noise_scalar = vec![0.0f32; len];

            let left_bg = 100.0;
            let right_bg = 200.0;
            let left_noise = 5.0;
            let right_noise = 10.0;
            let wx_start = 0.1;
            let wx_step = 0.8 / len as f32;

            interpolate_segment_simd(
                &mut bg_simd,
                &mut noise_simd,
                left_bg,
                right_bg,
                left_noise,
                right_noise,
                wx_start,
                wx_step,
            );

            interpolate_segment_scalar(
                &mut bg_scalar,
                &mut noise_scalar,
                left_bg,
                right_bg,
                left_noise,
                right_noise,
                wx_start,
                wx_step,
            );

            for i in 0..len {
                assert!(
                    (bg_simd[i] - bg_scalar[i]).abs() < 1e-4,
                    "len={}, i={}: bg mismatch {} vs {}",
                    len,
                    i,
                    bg_simd[i],
                    bg_scalar[i]
                );
                assert!(
                    (noise_simd[i] - noise_scalar[i]).abs() < 1e-4,
                    "len={}, i={}: noise mismatch {} vs {}",
                    len,
                    i,
                    noise_simd[i],
                    noise_scalar[i]
                );
            }
        }
    }

    #[test]
    fn test_interpolate_segment_simd_clamping() {
        // Test that weights outside [0,1] are clamped
        let mut bg = vec![0.0f32; 10];
        let mut noise = vec![0.0f32; 10];

        // wx_start = -0.5, ends at wx = -0.5 + 9*0.2 = 1.3
        // So first few should clamp to 0, last few should clamp to 1
        interpolate_segment_simd(&mut bg, &mut noise, 100.0, 200.0, 5.0, 10.0, -0.5, 0.2);

        // First element: wx = -0.5 clamped to 0 -> bg = 100
        assert!(
            (bg[0] - 100.0).abs() < 1e-4,
            "First element should be left value: {}",
            bg[0]
        );

        // Last element: wx = 1.3 clamped to 1 -> bg = 200
        assert!(
            (bg[9] - 200.0).abs() < 1e-4,
            "Last element should be right value: {}",
            bg[9]
        );
    }

    #[test]
    fn test_interpolate_segment_simd_constant_fill() {
        // When left == right, output should be constant
        let mut bg = vec![0.0f32; 50];
        let mut noise = vec![0.0f32; 50];

        interpolate_segment_simd(&mut bg, &mut noise, 150.0, 150.0, 7.5, 7.5, 0.0, 0.02);

        for (i, (&b, &n)) in bg.iter().zip(noise.iter()).enumerate() {
            assert!(
                (b - 150.0).abs() < 1e-5,
                "i={}: bg should be 150, got {}",
                i,
                b
            );
            assert!(
                (n - 7.5).abs() < 1e-5,
                "i={}: noise should be 7.5, got {}",
                i,
                n
            );
        }
    }
}
