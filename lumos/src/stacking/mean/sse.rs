//! SSE2 SIMD optimizations for mean stacking (x86_64).

use std::arch::x86_64::*;

/// Accumulate src into dst using SSE2 SIMD.
///
/// # Safety
/// Caller must ensure SSE2 is available (checked via is_x86_feature_detected).
#[target_feature(enable = "sse2")]
pub unsafe fn accumulate_chunk(dst: &mut [f32], src: &[f32]) {
    let chunks = dst.len() / 4;
    let remainder = dst.len() % 4;

    unsafe {
        for i in 0..chunks {
            let idx = i * 4;
            let d = _mm_loadu_ps(dst.as_ptr().add(idx));
            let s = _mm_loadu_ps(src.as_ptr().add(idx));
            let sum = _mm_add_ps(d, s);
            _mm_storeu_ps(dst.as_mut_ptr().add(idx), sum);
        }
    }

    // Handle remainder
    let start = chunks * 4;
    for i in 0..remainder {
        dst[start + i] += src[start + i];
    }
}

/// Divide all values by a scalar using SSE2 SIMD.
///
/// # Safety
/// Caller must ensure SSE2 is available (checked via is_x86_feature_detected).
#[target_feature(enable = "sse2")]
pub unsafe fn divide_chunk(data: &mut [f32], inv_count: f32) {
    let chunks = data.len() / 4;
    let remainder = data.len() % 4;

    unsafe {
        let inv_vec = _mm_set1_ps(inv_count);

        for i in 0..chunks {
            let idx = i * 4;
            let d = _mm_loadu_ps(data.as_ptr().add(idx));
            let result = _mm_mul_ps(d, inv_vec);
            _mm_storeu_ps(data.as_mut_ptr().add(idx), result);
        }
    }

    // Handle remainder
    let start = chunks * 4;
    for i in 0..remainder {
        data[start + i] *= inv_count;
    }
}
