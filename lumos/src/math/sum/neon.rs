//! NEON backends for the sum operations (aarch64).

use std::arch::aarch64::*;

use crate::math::sum::scalar;
use crate::math::sum::weighted_sums::WeightedSums;

/// Sum f32 values into an f64 accumulator, unrounded.
///
/// # Safety
/// None of its own. `dispatch!` calls every backend inside one `unsafe` block so the arms share a
/// signature, and on aarch64 NEON is unconditional — there is no feature for a caller to check.
pub(super) unsafe fn sum_f32(values: &[f32]) -> f64 {
    unsafe {
        let mut sum_lo = vdupq_n_f64(0.0);
        let mut sum_hi = vdupq_n_f64(0.0);

        let chunks = values.chunks_exact(4);
        let tail = chunks.remainder();

        for chunk in chunks {
            let v = vld1q_f32(chunk.as_ptr());
            sum_lo = vaddq_f64(sum_lo, vcvt_f64_f32(vget_low_f32(v)));
            sum_hi = vaddq_f64(sum_hi, vcvt_high_f64_f32(v));
        }

        vaddvq_f64(vaddq_f64(sum_lo, sum_hi)) + scalar::sum_f32(tail)
    }
}

/// Both weighted-mean totals over the same elements.
///
/// The lane split, the reduction order and the scalar tail match [`sum_f32`] exactly. That is what
/// makes `weighted_mean_f32` with unit weights reproduce `mean_f32` bit for bit: `v * 1.0` is exact,
/// so this walks the identical values through the identical accumulation and lands on the identical
/// f64 total.
///
/// # Safety
/// None of its own; see [`sum_f32`].
pub(super) unsafe fn weighted_sums(values: &[f32], weights: &[f32]) -> WeightedSums {
    unsafe {
        let mut weighted_lo = vdupq_n_f64(0.0);
        let mut weighted_hi = vdupq_n_f64(0.0);
        let mut weight_lo = vdupq_n_f64(0.0);
        let mut weight_hi = vdupq_n_f64(0.0);

        let value_chunks = values.chunks_exact(4);
        let weight_chunks = weights.chunks_exact(4);
        let value_tail = value_chunks.remainder();
        let weight_tail = weight_chunks.remainder();

        for (value_chunk, weight_chunk) in value_chunks.zip(weight_chunks) {
            let v = vld1q_f32(value_chunk.as_ptr());
            let w = vld1q_f32(weight_chunk.as_ptr());

            let v_lo = vcvt_f64_f32(vget_low_f32(v));
            let v_hi = vcvt_high_f64_f32(v);
            let w_lo = vcvt_f64_f32(vget_low_f32(w));
            let w_hi = vcvt_high_f64_f32(w);

            weighted_lo = vaddq_f64(weighted_lo, vmulq_f64(v_lo, w_lo));
            weighted_hi = vaddq_f64(weighted_hi, vmulq_f64(v_hi, w_hi));
            weight_lo = vaddq_f64(weight_lo, w_lo);
            weight_hi = vaddq_f64(weight_hi, w_hi);
        }

        let vector = WeightedSums {
            weighted_values: vaddvq_f64(vaddq_f64(weighted_lo, weighted_hi)),
            weight_total: vaddvq_f64(vaddq_f64(weight_lo, weight_hi)),
        };

        vector + scalar::weighted_sums(value_tail, weight_tail)
    }
}
