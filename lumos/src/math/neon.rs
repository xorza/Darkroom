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

/// Accumulate src into dst (dst[i] += src[i]) using NEON SIMD.
///
/// # Safety
/// Caller must ensure this runs on aarch64 with NEON support.
#[target_feature(enable = "neon")]
pub unsafe fn accumulate(dst: &mut [f32], src: &[f32]) {
    debug_assert_eq!(dst.len(), src.len());
    unsafe {
        let mut dst_chunks = dst.chunks_exact_mut(4);
        let mut src_chunks = src.chunks_exact(4);

        for (dst_chunk, src_chunk) in dst_chunks.by_ref().zip(src_chunks.by_ref()) {
            let d = vld1q_f32(dst_chunk.as_ptr());
            let s = vld1q_f32(src_chunk.as_ptr());
            let sum = vaddq_f32(d, s);
            vst1q_f32(dst_chunk.as_mut_ptr(), sum);
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

/// Scale values in-place (data[i] *= scale) using NEON SIMD.
///
/// # Safety
/// Caller must ensure this runs on aarch64 with NEON support.
#[target_feature(enable = "neon")]
pub unsafe fn scale(data: &mut [f32], scale_val: f32) {
    unsafe {
        let scale_vec = vdupq_n_f32(scale_val);
        let mut chunks = data.chunks_exact_mut(4);

        for chunk in chunks.by_ref() {
            let v = vld1q_f32(chunk.as_ptr());
            let scaled = vmulq_f32(v, scale_vec);
            vst1q_f32(chunk.as_mut_ptr(), scaled);
        }

        // Handle remainder
        for d in chunks.into_remainder() {
            *d *= scale_val;
        }
    }
}
