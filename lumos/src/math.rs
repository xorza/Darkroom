//! Math utilities with SIMD acceleration.
//!
//! Provides optimized math operations with platform-specific SIMD for ARM NEON (aarch64) and x86 SSE4.

/// Sum f32 values using SIMD when available.
#[cfg(target_arch = "aarch64")]
pub fn sum_f32(values: &[f32]) -> f32 {
    use std::arch::aarch64::*;

    if values.len() < 4 {
        return values.iter().sum();
    }

    unsafe {
        let mut sum_vec = vdupq_n_f32(0.0);
        let chunks = values.chunks_exact(4);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let v = vld1q_f32(chunk.as_ptr());
            sum_vec = vaddq_f32(sum_vec, v);
        }

        // Horizontal sum of the vector
        let sum = vaddvq_f32(sum_vec);

        // Add remainder
        sum + remainder.iter().sum::<f32>()
    }
}

/// Sum f32 values using SIMD when available.
#[cfg(target_arch = "x86_64")]
pub fn sum_f32(values: &[f32]) -> f32 {
    if values.len() < 4 || !is_x86_feature_detected!("sse4.1") {
        return values.iter().sum();
    }

    unsafe { sum_f32_sse(values) }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn sum_f32_sse(values: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let mut sum_vec = _mm_setzero_ps();
    let chunks = values.chunks_exact(4);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let v = _mm_loadu_ps(chunk.as_ptr());
        sum_vec = _mm_add_ps(sum_vec, v);
    }

    // Horizontal sum: [a, b, c, d] -> a + b + c + d
    let shuf = _mm_movehdup_ps(sum_vec); // [b, b, d, d]
    let sums = _mm_add_ps(sum_vec, shuf); // [a+b, b+b, c+d, d+d]
    let shuf = _mm_movehl_ps(sums, sums); // [c+d, d+d, c+d, d+d]
    let sums = _mm_add_ss(sums, shuf); // [a+b+c+d, ...]
    let sum = _mm_cvtss_f32(sums);

    sum + remainder.iter().sum::<f32>()
}

/// Fallback for other architectures.
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub fn sum_f32(values: &[f32]) -> f32 {
    values.iter().sum()
}

/// Calculate the mean of f32 values using SIMD-accelerated sum.
pub fn mean_f32(values: &[f32]) -> f32 {
    debug_assert!(!values.is_empty());
    sum_f32(values) / values.len() as f32
}

/// Calculate the median of f32 values.
pub fn median_f32(values: &[f32]) -> f32 {
    debug_assert!(!values.is_empty());

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let len = sorted.len();
    if len.is_multiple_of(2) {
        (sorted[len / 2 - 1] + sorted[len / 2]) / 2.0
    } else {
        sorted[len / 2]
    }
}

/// Calculate sum of squared differences from mean using SIMD.
#[cfg(target_arch = "aarch64")]
pub fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    use std::arch::aarch64::*;

    if values.len() < 4 {
        return values.iter().map(|v| (v - mean).powi(2)).sum();
    }

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

/// Calculate sum of squared differences from mean using SIMD.
#[cfg(target_arch = "x86_64")]
pub fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    if values.len() < 4 || !is_x86_feature_detected!("sse4.1") {
        return values.iter().map(|v| (v - mean).powi(2)).sum();
    }

    unsafe { sum_squared_diff_sse(values, mean) }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn sum_squared_diff_sse(values: &[f32], mean: f32) -> f32 {
    use std::arch::x86_64::*;

    let mean_vec = _mm_set1_ps(mean);
    let mut sum_vec = _mm_setzero_ps();
    let chunks = values.chunks_exact(4);
    let remainder = chunks.remainder();

    for chunk in chunks {
        let v = _mm_loadu_ps(chunk.as_ptr());
        let diff = _mm_sub_ps(v, mean_vec);
        let sq = _mm_mul_ps(diff, diff);
        sum_vec = _mm_add_ps(sum_vec, sq);
    }

    // Horizontal sum
    let shuf = _mm_movehdup_ps(sum_vec);
    let sums = _mm_add_ps(sum_vec, shuf);
    let shuf = _mm_movehl_ps(sums, sums);
    let sums = _mm_add_ss(sums, shuf);
    let sum = _mm_cvtss_f32(sums);

    sum + remainder.iter().map(|v| (v - mean).powi(2)).sum::<f32>()
}

/// Fallback for other architectures.
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    values.iter().map(|v| (v - mean).powi(2)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_median_odd() {
        let values = vec![1.0f32, 3.0, 2.0, 5.0, 4.0];
        assert!((median_f32(&values) - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_even() {
        let values = vec![1.0f32, 2.0, 3.0, 4.0];
        assert!((median_f32(&values) - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_f32() {
        let values: Vec<f32> = (1..=16).map(|x| x as f32).collect();
        let expected: f32 = values.iter().sum();
        let result = sum_f32(&values);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_f32_remainder() {
        // Test with non-multiple of 4 (exercises remainder handling)
        let values: Vec<f32> = (1..=13).map(|x| x as f32).collect();
        let expected: f32 = values.iter().sum();
        let result = sum_f32(&values);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_f32_small() {
        // Test with <4 elements (scalar fallback)
        let values = vec![1.0f32, 2.0, 3.0];
        let expected: f32 = values.iter().sum();
        let result = sum_f32(&values);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff() {
        let values: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let mean_val: f32 = 4.5;
        let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
        let result = sum_squared_diff(&values, mean_val);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff_remainder() {
        // Test with non-multiple of 4
        let values: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        let mean_val: f32 = 4.0;
        let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
        let result = sum_squared_diff(&values, mean_val);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff_small() {
        // Test with <4 elements (scalar fallback)
        let values: Vec<f32> = vec![1.0, 2.0, 3.0];
        let mean_val: f32 = 2.0;
        let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
        let result = sum_squared_diff(&values, mean_val);
        assert!((result - expected).abs() < 1e-4);
    }
}
