//! NEON SIMD optimizations for mean stacking (aarch64).

use std::arch::aarch64::*;

/// Accumulate src into dst using NEON SIMD.
///
/// # Safety
/// Caller must ensure this runs on aarch64 with NEON support.
#[target_feature(enable = "neon")]
pub unsafe fn accumulate_chunk(dst: &mut [f32], src: &[f32]) {
    let chunks = dst.len() / 4;
    let remainder = dst.len() % 4;

    unsafe {
        for i in 0..chunks {
            let idx = i * 4;
            let d = vld1q_f32(dst.as_ptr().add(idx));
            let s = vld1q_f32(src.as_ptr().add(idx));
            let sum = vaddq_f32(d, s);
            vst1q_f32(dst.as_mut_ptr().add(idx), sum);
        }
    }

    // Handle remainder
    let start = chunks * 4;
    for i in 0..remainder {
        dst[start + i] += src[start + i];
    }
}

/// Divide all values by a scalar using NEON SIMD.
///
/// # Safety
/// Caller must ensure this runs on aarch64 with NEON support.
#[target_feature(enable = "neon")]
pub unsafe fn divide_chunk(data: &mut [f32], inv_count: f32) {
    let chunks = data.len() / 4;
    let remainder = data.len() % 4;

    unsafe {
        let inv_vec = vdupq_n_f32(inv_count);

        for i in 0..chunks {
            let idx = i * 4;
            let d = vld1q_f32(data.as_ptr().add(idx));
            let result = vmulq_f32(d, inv_vec);
            vst1q_f32(data.as_mut_ptr().add(idx), result);
        }
    }

    // Handle remainder
    let start = chunks * 4;
    for i in 0..remainder {
        data[start + i] *= inv_count;
    }
}
