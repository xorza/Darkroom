//! SSE SIMD implementations of sum operations (x86_64).

use std::arch::x86_64::*;

/// Horizontal sum of a 4-element f32 vector: [a, b, c, d] -> a + b + c + d
#[inline]
#[target_feature(enable = "sse4.1")]
unsafe fn horizontal_sum(v: __m128) -> f32 {
    let shuf = _mm_movehdup_ps(v); // [b, b, d, d]
    let sums = _mm_add_ps(v, shuf); // [a+b, b+b, c+d, d+d]
    let shuf = _mm_movehl_ps(sums, sums); // [c+d, d+d, c+d, d+d]
    let sums = _mm_add_ss(sums, shuf); // [a+b+c+d, ...]
    _mm_cvtss_f32(sums)
}

/// Sum f32 values using SSE4.1 SIMD.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub unsafe fn sum_f32(values: &[f32]) -> f32 {
    unsafe {
        let mut sum_vec = _mm_setzero_ps();
        let chunks = values.chunks_exact(4);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let v = _mm_loadu_ps(chunk.as_ptr());
            sum_vec = _mm_add_ps(sum_vec, v);
        }

        horizontal_sum(sum_vec) + remainder.iter().sum::<f32>()
    }
}

/// Calculate sum of squared differences from mean using SSE4.1 SIMD.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub unsafe fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    unsafe {
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

        horizontal_sum(sum_vec)
            + remainder
                .iter()
                .map(|v| {
                    let diff = v - mean;
                    diff * diff
                })
                .sum::<f32>()
    }
}
