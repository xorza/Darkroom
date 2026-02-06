//! NEON SIMD implementations of sum operations (aarch64).

use std::arch::aarch64::*;

/// Sum f32 values using NEON SIMD.
///
/// # Safety
/// Caller must ensure NEON is available (always true on aarch64).
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

        // Horizontal sum
        let sum = vaddvq_f32(sum_vec);
        sum + remainder.iter().sum::<f32>()
    }
}

/// Calculate sum of squared differences from mean using NEON SIMD.
///
/// # Safety
/// Caller must ensure NEON is available (always true on aarch64).
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

        // Horizontal sum
        let sum = vaddvq_f32(sum_vec);
        sum + remainder
            .iter()
            .map(|v| {
                let diff = v - mean;
                diff * diff
            })
            .sum::<f32>()
    }
}
