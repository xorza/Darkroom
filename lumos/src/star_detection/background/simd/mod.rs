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

// ============================================================================
// Cubic spline segment interpolation
// ============================================================================

/// Natural cubic spline interpolation for a row segment using SIMD.
///
/// Evaluates f(t) = (1-t)*f0 + t*f1 - t*(1-t)*((2-t)*a + (1+t)*b)
/// where a = h²/6 * d2[left], b = h²/6 * d2[right].
///
/// # Arguments
/// * `bg_out` / `noise_out` - Output slices (same length)
/// * `bg_f0`, `bg_f1` - Background values at left/right tile centers
/// * `bg_a`, `bg_b` - Background spline correction terms
/// * `noise_f0`, `noise_f1` - Noise values at left/right tile centers
/// * `noise_a`, `noise_b` - Noise spline correction terms
/// * `tx_start` - Starting t parameter (0.0 at left center)
/// * `tx_step` - t increment per pixel
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn interpolate_segment_cubic_simd(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg_f0: f32,
    bg_f1: f32,
    bg_a: f32,
    bg_b: f32,
    noise_f0: f32,
    noise_f1: f32,
    noise_a: f32,
    noise_b: f32,
    tx_start: f32,
    tx_step: f32,
) {
    debug_assert_eq!(bg_out.len(), noise_out.len());

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            unsafe {
                interpolate_segment_cubic_avx2(
                    bg_out, noise_out, bg_f0, bg_f1, bg_a, bg_b, noise_f0, noise_f1, noise_a,
                    noise_b, tx_start, tx_step,
                );
            }
            return;
        }
        if cpu_features::has_sse4_1() {
            unsafe {
                interpolate_segment_cubic_sse(
                    bg_out, noise_out, bg_f0, bg_f1, bg_a, bg_b, noise_f0, noise_f1, noise_a,
                    noise_b, tx_start, tx_step,
                );
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            interpolate_segment_cubic_neon(
                bg_out, noise_out, bg_f0, bg_f1, bg_a, bg_b, noise_f0, noise_f1, noise_a, noise_b,
                tx_start, tx_step,
            );
        }
        return;
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    interpolate_segment_cubic_scalar(
        bg_out, noise_out, bg_f0, bg_f1, bg_a, bg_b, noise_f0, noise_f1, noise_a, noise_b,
        tx_start, tx_step,
    );
}

/// Evaluate cubic spline for a single value.
///
/// f(t) = (1-t)*f0 + t*f1 - t*(1-t)*((2-t)*a + (1+t)*b)
#[inline]
fn cubic_eval(f0: f32, f1: f32, a: f32, b: f32, t: f32) -> f32 {
    let ct = 1.0 - t;
    let t_ct = t * ct;
    ct * f0 + t * f1 - t_ct * ((2.0 - t) * a + (1.0 + t) * b)
}

/// Scalar implementation of cubic spline segment interpolation.
#[allow(clippy::too_many_arguments)]
#[inline]
fn interpolate_segment_cubic_scalar(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg_f0: f32,
    bg_f1: f32,
    bg_a: f32,
    bg_b: f32,
    noise_f0: f32,
    noise_f1: f32,
    noise_a: f32,
    noise_b: f32,
    tx_start: f32,
    tx_step: f32,
) {
    for (i, (bg, noise)) in bg_out.iter_mut().zip(noise_out.iter_mut()).enumerate() {
        let t = (tx_start + i as f32 * tx_step).clamp(0.0, 1.0);
        *bg = cubic_eval(bg_f0, bg_f1, bg_a, bg_b, t);
        *noise = cubic_eval(noise_f0, noise_f1, noise_a, noise_b, t);
    }
}

// ============================================================================
// Cubic spline SIMD implementations
// ============================================================================

/// Evaluate cubic spline for 8 values using AVX2+FMA.
///
/// f(t) = (1-t)*f0 + t*f1 - t*(1-t)*((2-t)*a + (1+t)*b)
#[cfg(target_arch = "x86_64")]
#[allow(clippy::too_many_arguments)]
#[target_feature(enable = "avx2,fma")]
unsafe fn interpolate_segment_cubic_avx2(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg_f0: f32,
    bg_f1: f32,
    bg_a: f32,
    bg_b: f32,
    noise_f0: f32,
    noise_f1: f32,
    noise_a: f32,
    noise_b: f32,
    tx_start: f32,
    tx_step: f32,
) {
    use std::arch::x86_64::*;

    let len = bg_out.len();

    unsafe {
        let bg_f0_v = _mm256_set1_ps(bg_f0);
        let bg_f1_v = _mm256_set1_ps(bg_f1);
        let bg_a_v = _mm256_set1_ps(bg_a);
        let bg_b_v = _mm256_set1_ps(bg_b);
        let noise_f0_v = _mm256_set1_ps(noise_f0);
        let noise_f1_v = _mm256_set1_ps(noise_f1);
        let noise_a_v = _mm256_set1_ps(noise_a);
        let noise_b_v = _mm256_set1_ps(noise_b);
        let one = _mm256_set1_ps(1.0);
        let two = _mm256_set1_ps(2.0);
        let zero = _mm256_setzero_ps();
        let step8 = _mm256_set1_ps(tx_step * 8.0);

        let mut t_v = _mm256_set_ps(
            tx_start + 7.0 * tx_step,
            tx_start + 6.0 * tx_step,
            tx_start + 5.0 * tx_step,
            tx_start + 4.0 * tx_step,
            tx_start + 3.0 * tx_step,
            tx_start + 2.0 * tx_step,
            tx_start + tx_step,
            tx_start,
        );

        let mut i = 0;
        while i + 8 <= len {
            let t = _mm256_min_ps(_mm256_max_ps(t_v, zero), one);
            let ct = _mm256_sub_ps(one, t);

            // cubic = (2-t)*a + (1+t)*b
            let two_minus_t = _mm256_sub_ps(two, t);
            let one_plus_t = _mm256_add_ps(one, t);
            let cubic = _mm256_fmadd_ps(one_plus_t, bg_b_v, _mm256_mul_ps(two_minus_t, bg_a_v));
            // result = ct*f0 + t*f1 - t*ct*cubic
            let t_ct = _mm256_mul_ps(t, ct);
            let linear = _mm256_fmadd_ps(t, bg_f1_v, _mm256_mul_ps(ct, bg_f0_v));
            let result = _mm256_fnmadd_ps(t_ct, cubic, linear);
            _mm256_storeu_ps(bg_out.as_mut_ptr().add(i), result);

            // Same for noise
            let n_cubic =
                _mm256_fmadd_ps(one_plus_t, noise_b_v, _mm256_mul_ps(two_minus_t, noise_a_v));
            let n_linear = _mm256_fmadd_ps(t, noise_f1_v, _mm256_mul_ps(ct, noise_f0_v));
            let n_result = _mm256_fnmadd_ps(t_ct, n_cubic, n_linear);
            _mm256_storeu_ps(noise_out.as_mut_ptr().add(i), n_result);

            t_v = _mm256_add_ps(t_v, step8);
            i += 8;
        }

        // SSE remainder (4 at a time)
        if i + 4 <= len {
            let bg_f0_4 = _mm_set1_ps(bg_f0);
            let bg_f1_4 = _mm_set1_ps(bg_f1);
            let bg_a_4 = _mm_set1_ps(bg_a);
            let bg_b_4 = _mm_set1_ps(bg_b);
            let noise_f0_4 = _mm_set1_ps(noise_f0);
            let noise_f1_4 = _mm_set1_ps(noise_f1);
            let noise_a_4 = _mm_set1_ps(noise_a);
            let noise_b_4 = _mm_set1_ps(noise_b);
            let one4 = _mm_set1_ps(1.0);
            let two4 = _mm_set1_ps(2.0);
            let zero4 = _mm_setzero_ps();

            let cur = tx_start + i as f32 * tx_step;
            let t4 = _mm_min_ps(
                _mm_max_ps(
                    _mm_set_ps(cur + 3.0 * tx_step, cur + 2.0 * tx_step, cur + tx_step, cur),
                    zero4,
                ),
                one4,
            );
            let ct4 = _mm_sub_ps(one4, t4);
            let two_minus_t4 = _mm_sub_ps(two4, t4);
            let one_plus_t4 = _mm_add_ps(one4, t4);

            let cubic4 = _mm_fmadd_ps(one_plus_t4, bg_b_4, _mm_mul_ps(two_minus_t4, bg_a_4));
            let t_ct4 = _mm_mul_ps(t4, ct4);
            let lin4 = _mm_fmadd_ps(t4, bg_f1_4, _mm_mul_ps(ct4, bg_f0_4));
            let r4 = _mm_fnmadd_ps(t_ct4, cubic4, lin4);
            _mm_storeu_ps(bg_out.as_mut_ptr().add(i), r4);

            let nc4 = _mm_fmadd_ps(one_plus_t4, noise_b_4, _mm_mul_ps(two_minus_t4, noise_a_4));
            let nlin4 = _mm_fmadd_ps(t4, noise_f1_4, _mm_mul_ps(ct4, noise_f0_4));
            let nr4 = _mm_fnmadd_ps(t_ct4, nc4, nlin4);
            _mm_storeu_ps(noise_out.as_mut_ptr().add(i), nr4);
            i += 4;
        }

        // Scalar remainder
        while i < len {
            let t = (tx_start + i as f32 * tx_step).clamp(0.0, 1.0);
            bg_out[i] = cubic_eval(bg_f0, bg_f1, bg_a, bg_b, t);
            noise_out[i] = cubic_eval(noise_f0, noise_f1, noise_a, noise_b, t);
            i += 1;
        }
    }
}

/// Evaluate cubic spline for 4 values using SSE4.1.
#[cfg(target_arch = "x86_64")]
#[allow(clippy::too_many_arguments)]
#[target_feature(enable = "sse4.1")]
unsafe fn interpolate_segment_cubic_sse(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg_f0: f32,
    bg_f1: f32,
    bg_a: f32,
    bg_b: f32,
    noise_f0: f32,
    noise_f1: f32,
    noise_a: f32,
    noise_b: f32,
    tx_start: f32,
    tx_step: f32,
) {
    use std::arch::x86_64::*;

    let len = bg_out.len();

    unsafe {
        let bg_f0_v = _mm_set1_ps(bg_f0);
        let bg_f1_v = _mm_set1_ps(bg_f1);
        let bg_a_v = _mm_set1_ps(bg_a);
        let bg_b_v = _mm_set1_ps(bg_b);
        let noise_f0_v = _mm_set1_ps(noise_f0);
        let noise_f1_v = _mm_set1_ps(noise_f1);
        let noise_a_v = _mm_set1_ps(noise_a);
        let noise_b_v = _mm_set1_ps(noise_b);
        let one = _mm_set1_ps(1.0);
        let two = _mm_set1_ps(2.0);
        let zero = _mm_setzero_ps();
        let step4 = _mm_set1_ps(tx_step * 4.0);

        let mut t_v = _mm_set_ps(
            tx_start + 3.0 * tx_step,
            tx_start + 2.0 * tx_step,
            tx_start + tx_step,
            tx_start,
        );

        let mut i = 0;
        while i + 4 <= len {
            let t = _mm_min_ps(_mm_max_ps(t_v, zero), one);
            let ct = _mm_sub_ps(one, t);

            // cubic = (2-t)*a + (1+t)*b (no FMA on SSE4.1)
            let two_minus_t = _mm_sub_ps(two, t);
            let one_plus_t = _mm_add_ps(one, t);
            let cubic = _mm_add_ps(
                _mm_mul_ps(two_minus_t, bg_a_v),
                _mm_mul_ps(one_plus_t, bg_b_v),
            );
            let t_ct = _mm_mul_ps(t, ct);
            // result = ct*f0 + t*f1 - t*ct*cubic
            let linear = _mm_add_ps(_mm_mul_ps(ct, bg_f0_v), _mm_mul_ps(t, bg_f1_v));
            let result = _mm_sub_ps(linear, _mm_mul_ps(t_ct, cubic));
            _mm_storeu_ps(bg_out.as_mut_ptr().add(i), result);

            let n_cubic = _mm_add_ps(
                _mm_mul_ps(two_minus_t, noise_a_v),
                _mm_mul_ps(one_plus_t, noise_b_v),
            );
            let n_linear = _mm_add_ps(_mm_mul_ps(ct, noise_f0_v), _mm_mul_ps(t, noise_f1_v));
            let n_result = _mm_sub_ps(n_linear, _mm_mul_ps(t_ct, n_cubic));
            _mm_storeu_ps(noise_out.as_mut_ptr().add(i), n_result);

            t_v = _mm_add_ps(t_v, step4);
            i += 4;
        }

        // Scalar remainder
        while i < len {
            let t = (tx_start + i as f32 * tx_step).clamp(0.0, 1.0);
            bg_out[i] = cubic_eval(bg_f0, bg_f1, bg_a, bg_b, t);
            noise_out[i] = cubic_eval(noise_f0, noise_f1, noise_a, noise_b, t);
            i += 1;
        }
    }
}

/// Evaluate cubic spline for 4 values using NEON.
#[cfg(target_arch = "aarch64")]
#[allow(clippy::too_many_arguments)]
unsafe fn interpolate_segment_cubic_neon(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg_f0: f32,
    bg_f1: f32,
    bg_a: f32,
    bg_b: f32,
    noise_f0: f32,
    noise_f1: f32,
    noise_a: f32,
    noise_b: f32,
    tx_start: f32,
    tx_step: f32,
) {
    use std::arch::aarch64::*;

    let len = bg_out.len();

    unsafe {
        let bg_f0_v = vdupq_n_f32(bg_f0);
        let bg_f1_v = vdupq_n_f32(bg_f1);
        let bg_a_v = vdupq_n_f32(bg_a);
        let bg_b_v = vdupq_n_f32(bg_b);
        let noise_f0_v = vdupq_n_f32(noise_f0);
        let noise_f1_v = vdupq_n_f32(noise_f1);
        let noise_a_v = vdupq_n_f32(noise_a);
        let noise_b_v = vdupq_n_f32(noise_b);
        let one = vdupq_n_f32(1.0);
        let two = vdupq_n_f32(2.0);
        let zero = vdupq_n_f32(0.0);
        let step4 = vdupq_n_f32(tx_step * 4.0);

        let offsets: [f32; 4] = [0.0, tx_step, 2.0 * tx_step, 3.0 * tx_step];
        let mut t_v = vaddq_f32(vdupq_n_f32(tx_start), vld1q_f32(offsets.as_ptr()));

        let mut i = 0;
        while i + 4 <= len {
            let t = vminq_f32(vmaxq_f32(t_v, zero), one);
            let ct = vsubq_f32(one, t);

            // cubic = (2-t)*a + (1+t)*b
            let two_minus_t = vsubq_f32(two, t);
            let one_plus_t = vaddq_f32(one, t);
            let cubic = vfmaq_f32(vmulq_f32(two_minus_t, bg_a_v), one_plus_t, bg_b_v);
            let t_ct = vmulq_f32(t, ct);
            // result = ct*f0 + t*f1 - t*ct*cubic
            let linear = vfmaq_f32(vmulq_f32(ct, bg_f0_v), t, bg_f1_v);
            let result = vsubq_f32(linear, vmulq_f32(t_ct, cubic));
            vst1q_f32(bg_out.as_mut_ptr().add(i), result);

            let n_cubic = vfmaq_f32(vmulq_f32(two_minus_t, noise_a_v), one_plus_t, noise_b_v);
            let n_linear = vfmaq_f32(vmulq_f32(ct, noise_f0_v), t, noise_f1_v);
            let n_result = vsubq_f32(n_linear, vmulq_f32(t_ct, n_cubic));
            vst1q_f32(noise_out.as_mut_ptr().add(i), n_result);

            t_v = vaddq_f32(t_v, step4);
            i += 4;
        }

        // Scalar remainder
        while i < len {
            let t = (tx_start + i as f32 * tx_step).clamp(0.0, 1.0);
            bg_out[i] = cubic_eval(bg_f0, bg_f1, bg_a, bg_b, t);
            noise_out[i] = cubic_eval(noise_f0, noise_f1, noise_a, noise_b, t);
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

    // ========== Cubic spline interpolation SIMD tests ==========

    #[test]
    fn test_cubic_segment_simd_matches_scalar() {
        // Test various segment lengths including SIMD boundary cases
        for len in [1, 3, 4, 7, 8, 15, 16, 31, 64, 100] {
            let mut bg_simd = vec![0.0f32; len];
            let mut noise_simd = vec![0.0f32; len];
            let mut bg_scalar = vec![0.0f32; len];
            let mut noise_scalar = vec![0.0f32; len];

            // Non-trivial spline parameters
            let bg_f0 = 100.0;
            let bg_f1 = 200.0;
            let bg_a = -5.0; // = -h²/6 * d2_left
            let bg_b = 3.0; // = -h²/6 * d2_right
            let noise_f0 = 5.0;
            let noise_f1 = 10.0;
            let noise_a = -0.5;
            let noise_b = 0.3;
            let tx_start = 0.1;
            let tx_step = 0.8 / len as f32;

            interpolate_segment_cubic_simd(
                &mut bg_simd,
                &mut noise_simd,
                bg_f0,
                bg_f1,
                bg_a,
                bg_b,
                noise_f0,
                noise_f1,
                noise_a,
                noise_b,
                tx_start,
                tx_step,
            );

            interpolate_segment_cubic_scalar(
                &mut bg_scalar,
                &mut noise_scalar,
                bg_f0,
                bg_f1,
                bg_a,
                bg_b,
                noise_f0,
                noise_f1,
                noise_a,
                noise_b,
                tx_start,
                tx_step,
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
    fn test_cubic_segment_simd_endpoints() {
        // At t=0 result should be f0, at t=1 result should be f1
        // (regardless of a, b coefficients, since t*(1-t) = 0 at both endpoints)
        let mut bg = vec![0.0f32; 2];
        let mut noise = vec![0.0f32; 2];

        // t=0 for first pixel, t=1 for second pixel
        interpolate_segment_cubic_simd(
            &mut bg, &mut noise, 100.0, 200.0, -10.0, 7.0, 5.0, 15.0, -1.0, 0.5, 0.0, 1.0,
        );

        // f(0) = 1*100 + 0*200 + 0*1*(1*a + 0*b) = 100
        assert!(
            (bg[0] - 100.0).abs() < 1e-4,
            "t=0: bg should be f0=100, got {}",
            bg[0]
        );
        // f(1) = 0*100 + 1*200 + 1*0*(0*a + 1*b) = 200
        assert!(
            (bg[1] - 200.0).abs() < 1e-4,
            "t=1: bg should be f1=200, got {}",
            bg[1]
        );
        assert!(
            (noise[0] - 5.0).abs() < 1e-4,
            "t=0: noise should be f0=5, got {}",
            noise[0]
        );
        assert!(
            (noise[1] - 15.0).abs() < 1e-4,
            "t=1: noise should be f1=15, got {}",
            noise[1]
        );
    }

    #[test]
    fn test_cubic_segment_simd_midpoint() {
        // At t=0.5, using f(t) = ct*f0 + t*f1 - t*ct*((2-t)*a + (1+t)*b):
        //   = 0.5*f0 + 0.5*f1 - 0.5*0.5*(1.5*a + 1.5*b)
        //   = (f0+f1)/2 - 0.375*(a+b)
        let mut bg = vec![0.0f32; 1];
        let mut noise = vec![0.0f32; 1];

        let f0 = 100.0;
        let f1 = 200.0;
        let a = -8.0;
        let b = 16.0;
        // Expected: (100+200)/2 - 0.375*(-8+16) = 150 - 3 = 147
        interpolate_segment_cubic_simd(
            &mut bg, &mut noise, f0, f1, a, b, 0.0, 0.0, 0.0, 0.0, 0.5, 1.0,
        );

        assert!(
            (bg[0] - 147.0).abs() < 1e-4,
            "Midpoint: expected 147, got {}",
            bg[0]
        );
    }

    #[test]
    fn test_cubic_segment_simd_linear_when_no_correction() {
        // With a=0, b=0, cubic spline reduces to linear interpolation
        let mut bg = vec![0.0f32; 50];
        let mut noise = vec![0.0f32; 50];

        let f0 = 100.0;
        let f1 = 200.0;
        let tx_step = 1.0 / 49.0;

        interpolate_segment_cubic_simd(
            &mut bg, &mut noise, f0, f1, 0.0, 0.0, 5.0, 10.0, 0.0, 0.0, 0.0, tx_step,
        );

        for (i, &b) in bg.iter().enumerate() {
            let t = (i as f32 * tx_step).clamp(0.0, 1.0);
            let expected = (1.0 - t) * f0 + t * f1;
            assert!(
                (b - expected).abs() < 1e-3,
                "i={}: expected linear {}, got {}",
                i,
                expected,
                b
            );
        }
    }

    #[test]
    fn test_cubic_segment_simd_clamping() {
        // t values outside [0,1] should be clamped
        let mut bg = vec![0.0f32; 10];
        let mut noise = vec![0.0f32; 10];

        // tx_start = -0.5, step = 0.2 → t goes from -0.5 to 1.3
        interpolate_segment_cubic_simd(
            &mut bg, &mut noise, 100.0, 200.0, -5.0, 3.0, 5.0, 10.0, -0.5, 0.3, -0.5, 0.2,
        );

        // First element: t = -0.5 clamped to 0 → bg = f0 = 100
        assert!(
            (bg[0] - 100.0).abs() < 1e-4,
            "t<0 clamped: expected f0=100, got {}",
            bg[0]
        );

        // Last element: t = -0.5 + 9*0.2 = 1.3 clamped to 1 → bg = f1 = 200
        assert!(
            (bg[9] - 200.0).abs() < 1e-4,
            "t>1 clamped: expected f1=200, got {}",
            bg[9]
        );
    }
}
