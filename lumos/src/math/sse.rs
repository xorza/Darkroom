//! SSE SIMD implementations of math operations (x86_64).

use std::arch::x86_64::*;

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

        // Horizontal sum: [a, b, c, d] -> a + b + c + d
        let shuf = _mm_movehdup_ps(sum_vec); // [b, b, d, d]
        let sums = _mm_add_ps(sum_vec, shuf); // [a+b, b+b, c+d, d+d]
        let shuf = _mm_movehl_ps(sums, sums); // [c+d, d+d, c+d, d+d]
        let sums = _mm_add_ss(sums, shuf); // [a+b+c+d, ...]
        let sum = _mm_cvtss_f32(sums);

        sum + remainder.iter().sum::<f32>()
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

        // Horizontal sum
        let shuf = _mm_movehdup_ps(sum_vec);
        let sums = _mm_add_ps(sum_vec, shuf);
        let shuf = _mm_movehl_ps(sums, sums);
        let sums = _mm_add_ss(sums, shuf);
        let sum = _mm_cvtss_f32(sums);

        sum + remainder.iter().map(|v| (v - mean).powi(2)).sum::<f32>()
    }
}

/// Accumulate src into dst (dst[i] += src[i]) using SSE4.1 SIMD.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub unsafe fn accumulate(dst: &mut [f32], src: &[f32]) {
    debug_assert_eq!(dst.len(), src.len());
    unsafe {
        let mut dst_chunks = dst.chunks_exact_mut(4);
        let mut src_chunks = src.chunks_exact(4);

        for (dst_chunk, src_chunk) in dst_chunks.by_ref().zip(src_chunks.by_ref()) {
            let d = _mm_loadu_ps(dst_chunk.as_ptr());
            let s = _mm_loadu_ps(src_chunk.as_ptr());
            let sum = _mm_add_ps(d, s);
            _mm_storeu_ps(dst_chunk.as_mut_ptr(), sum);
        }

        // Handle remainder
        for (d, &s) in dst_chunks
            .into_remainder()
            .iter_mut()
            .zip(src_chunks.remainder())
        {
            *d += s;
        }
    }
}

/// Scale values in-place (data[i] *= scale) using SSE4.1 SIMD.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub unsafe fn scale(data: &mut [f32], scale_val: f32) {
    unsafe {
        let scale_vec = _mm_set1_ps(scale_val);
        let mut chunks = data.chunks_exact_mut(4);

        for chunk in chunks.by_ref() {
            let v = _mm_loadu_ps(chunk.as_ptr());
            let scaled = _mm_mul_ps(v, scale_vec);
            _mm_storeu_ps(chunk.as_mut_ptr(), scaled);
        }

        // Handle remainder
        for d in chunks.into_remainder() {
            *d *= scale_val;
        }
    }
}
