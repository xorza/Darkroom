//! SSE4.1 and AVX2 implementations of background statistics functions.

#![allow(clippy::needless_range_loop)]
#![allow(dead_code)]

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Compute sum and sum of squares using AVX2.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn sum_and_sum_sq_avx2(values: &[f32]) -> (f32, f32) {
    unsafe {
        let len = values.len();
        let ptr = values.as_ptr();

        let mut sum_vec = _mm256_setzero_ps();
        let mut sum_sq_vec = _mm256_setzero_ps();

        // Process 8 floats at a time
        let chunks = len / 8;
        for i in 0..chunks {
            let v = _mm256_loadu_ps(ptr.add(i * 8));
            sum_vec = _mm256_add_ps(sum_vec, v);
            sum_sq_vec = _mm256_add_ps(sum_sq_vec, _mm256_mul_ps(v, v));
        }

        // Horizontal sum of vectors
        let sum = horizontal_sum_avx2(sum_vec);
        let sum_sq = horizontal_sum_avx2(sum_sq_vec);

        // Handle remainder
        let remainder_start = chunks * 8;
        let mut sum_rem = 0.0f32;
        let mut sum_sq_rem = 0.0f32;
        for i in remainder_start..len {
            let v = *ptr.add(i);
            sum_rem += v;
            sum_sq_rem += v * v;
        }

        (sum + sum_rem, sum_sq + sum_sq_rem)
    }
}

/// Horizontal sum of 8 floats in an AVX2 vector.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[inline]
unsafe fn horizontal_sum_avx2(v: __m256) -> f32 {
    // Sum pairs: [a0+a4, a1+a5, a2+a6, a3+a7, ...]
    let hi = _mm256_extractf128_ps(v, 1);
    let lo = _mm256_castps256_ps128(v);
    let sum128 = _mm_add_ps(lo, hi);

    // Now sum the 4-element SSE vector
    let hi64 = _mm_movehl_ps(sum128, sum128);
    let sum64 = _mm_add_ps(sum128, hi64);
    let hi32 = _mm_shuffle_ps(sum64, sum64, 0x55);
    let sum32 = _mm_add_ss(sum64, hi32);

    _mm_cvtss_f32(sum32)
}

/// Compute sum and sum of squares using SSE4.1.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
pub unsafe fn sum_and_sum_sq_sse41(values: &[f32]) -> (f32, f32) {
    unsafe {
        let len = values.len();
        let ptr = values.as_ptr();

        let mut sum_vec = _mm_setzero_ps();
        let mut sum_sq_vec = _mm_setzero_ps();

        // Process 4 floats at a time
        let chunks = len / 4;
        for i in 0..chunks {
            let v = _mm_loadu_ps(ptr.add(i * 4));
            sum_vec = _mm_add_ps(sum_vec, v);
            sum_sq_vec = _mm_add_ps(sum_sq_vec, _mm_mul_ps(v, v));
        }

        // Horizontal sum
        let sum = horizontal_sum_sse(sum_vec);
        let sum_sq = horizontal_sum_sse(sum_sq_vec);

        // Handle remainder
        let remainder_start = chunks * 4;
        let mut sum_rem = 0.0f32;
        let mut sum_sq_rem = 0.0f32;
        for i in remainder_start..len {
            let v = *ptr.add(i);
            sum_rem += v;
            sum_sq_rem += v * v;
        }

        (sum + sum_rem, sum_sq + sum_sq_rem)
    }
}

/// Horizontal sum of 4 floats in an SSE vector.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
#[inline]
unsafe fn horizontal_sum_sse(v: __m128) -> f32 {
    let hi64 = _mm_movehl_ps(v, v);
    let sum64 = _mm_add_ps(v, hi64);
    let hi32 = _mm_shuffle_ps(sum64, sum64, 0x55);
    let sum32 = _mm_add_ss(sum64, hi32);
    _mm_cvtss_f32(sum32)
}

/// Compute sum of absolute deviations from median using AVX2.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn sum_abs_deviations_avx2(values: &[f32], median: f32) -> f32 {
    unsafe {
        let len = values.len();
        let ptr = values.as_ptr();

        let median_vec = _mm256_set1_ps(median);
        let sign_mask = _mm256_set1_ps(-0.0); // Mask for absolute value
        let mut sum_vec = _mm256_setzero_ps();

        // Process 8 floats at a time
        let chunks = len / 8;
        for i in 0..chunks {
            let v = _mm256_loadu_ps(ptr.add(i * 8));
            let diff = _mm256_sub_ps(v, median_vec);
            let abs_diff = _mm256_andnot_ps(sign_mask, diff); // Clear sign bit
            sum_vec = _mm256_add_ps(sum_vec, abs_diff);
        }

        let mut sum = horizontal_sum_avx2(sum_vec);

        // Handle remainder
        let remainder_start = chunks * 8;
        for i in remainder_start..len {
            sum += (*ptr.add(i) - median).abs();
        }

        sum
    }
}

/// Compute sum of absolute deviations from median using SSE4.1.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
pub unsafe fn sum_abs_deviations_sse41(values: &[f32], median: f32) -> f32 {
    unsafe {
        let len = values.len();
        let ptr = values.as_ptr();

        let median_vec = _mm_set1_ps(median);
        let sign_mask = _mm_set1_ps(-0.0);
        let mut sum_vec = _mm_setzero_ps();

        // Process 4 floats at a time
        let chunks = len / 4;
        for i in 0..chunks {
            let v = _mm_loadu_ps(ptr.add(i * 4));
            let diff = _mm_sub_ps(v, median_vec);
            let abs_diff = _mm_andnot_ps(sign_mask, diff);
            sum_vec = _mm_add_ps(sum_vec, abs_diff);
        }

        let mut sum = horizontal_sum_sse(sum_vec);

        // Handle remainder
        let remainder_start = chunks * 4;
        for i in remainder_start..len {
            sum += (*ptr.add(i) - median).abs();
        }

        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_arch = "x86_64")]
    use common::cpu_features;

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_sum_and_sum_sq() {
        if !cpu_features::has_avx2() {
            eprintln!("Skipping AVX2 test");
            return;
        }

        let values: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let (sum, sum_sq) = unsafe { sum_and_sum_sq_avx2(&values) };

        let expected_sum: f32 = (1..=100).map(|i| i as f32).sum();
        let expected_sum_sq: f32 = (1..=100).map(|i| (i * i) as f32).sum();

        assert!(
            (sum - expected_sum).abs() < 1e-3,
            "Sum: {} vs {}",
            sum,
            expected_sum
        );
        assert!(
            (sum_sq - expected_sum_sq).abs() < 1.0,
            "Sum sq: {} vs {}",
            sum_sq,
            expected_sum_sq
        );
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_sse41_sum_and_sum_sq() {
        if !cpu_features::has_sse4_1() {
            eprintln!("Skipping SSE4.1 test");
            return;
        }

        let values: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let (sum, sum_sq) = unsafe { sum_and_sum_sq_sse41(&values) };

        let expected_sum: f32 = (1..=100).map(|i| i as f32).sum();
        let expected_sum_sq: f32 = (1..=100).map(|i| (i * i) as f32).sum();

        assert!(
            (sum - expected_sum).abs() < 1e-3,
            "Sum: {} vs {}",
            sum,
            expected_sum
        );
        assert!(
            (sum_sq - expected_sum_sq).abs() < 1.0,
            "Sum sq: {} vs {}",
            sum_sq,
            expected_sum_sq
        );
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_sum_abs_deviations() {
        if !cpu_features::has_avx2() {
            eprintln!("Skipping AVX2 test");
            return;
        }

        let values: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let median = 50.5;
        let sum = unsafe { sum_abs_deviations_avx2(&values, median) };

        let expected: f32 = values.iter().map(|&v| (v - median).abs()).sum();

        assert!(
            (sum - expected).abs() < 0.1,
            "Sum abs dev: {} vs {}",
            sum,
            expected
        );
    }
}
