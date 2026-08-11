//! NEON SIMD implementations of sum operations (aarch64).

use std::arch::aarch64::*;

/// Sum f32 values using NEON SIMD, accumulating in f64.
///
/// Widening each lane costs one `vcvt` per half and buys the same result
/// [`crate::math::sum::scalar::sum_f32`] produces — the f64 sum rounded once — where compensating
/// in f32 only approached it. See the note on [`crate::math::sum::simd::sum_f32`] for why the
/// wider accumulator is both the accurate and the fast choice.
///
/// # Safety
/// Caller must ensure NEON is available (always true on aarch64).
pub(super) unsafe fn sum_f32(values: &[f32]) -> f32 {
    unsafe {
        let mut sum_lo = vdupq_n_f64(0.0);
        let mut sum_hi = vdupq_n_f64(0.0);

        let chunks = values.chunks_exact(4);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let v = vld1q_f32(chunk.as_ptr());
            sum_lo = vaddq_f64(sum_lo, vcvt_f64_f32(vget_low_f32(v)));
            sum_hi = vaddq_f64(sum_hi, vcvt_high_f64_f32(v));
        }

        let mut total = vaddvq_f64(vaddq_f64(sum_lo, sum_hi));

        for &v in remainder {
            total += f64::from(v);
        }

        total as f32
    }
}

/// Weighted mean using NEON SIMD, accumulating both sums in f64.
///
/// Widening each lane to f64 rather than compensating in f32 is what keeps this agreeing with
/// [`crate::math::sum::mean_f32`] on the same pixel — see the module note on
/// [`crate::math::sum::simd::weighted_mean_f32`]. Both conversions are lossless and the f64
/// product of two f32s is exact, so this accumulates the same terms as
/// [`crate::math::sum::scalar::weighted_mean_f32`], only in a different order.
///
/// # Safety
/// Caller must ensure NEON is available (always true on aarch64).
pub(super) unsafe fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    unsafe {
        let mut sum_vw_lo = vdupq_n_f64(0.0);
        let mut sum_vw_hi = vdupq_n_f64(0.0);
        let mut sum_w_lo = vdupq_n_f64(0.0);
        let mut sum_w_hi = vdupq_n_f64(0.0);

        let v_chunks = values.chunks_exact(4);
        let v_rem = v_chunks.remainder();
        let mut w_ptr = weights.as_ptr();

        for v_chunk in v_chunks {
            let v = vld1q_f32(v_chunk.as_ptr());
            let w = vld1q_f32(w_ptr);
            w_ptr = w_ptr.add(4);

            let v_lo = vcvt_f64_f32(vget_low_f32(v));
            let v_hi = vcvt_high_f64_f32(v);
            let w_lo = vcvt_f64_f32(vget_low_f32(w));
            let w_hi = vcvt_high_f64_f32(w);

            sum_vw_lo = vaddq_f64(sum_vw_lo, vmulq_f64(v_lo, w_lo));
            sum_vw_hi = vaddq_f64(sum_vw_hi, vmulq_f64(v_hi, w_hi));
            sum_w_lo = vaddq_f64(sum_w_lo, w_lo);
            sum_w_hi = vaddq_f64(sum_w_hi, w_hi);
        }

        let mut total_vw = vaddvq_f64(vaddq_f64(sum_vw_lo, sum_vw_hi));
        let mut total_w = vaddvq_f64(vaddq_f64(sum_w_lo, sum_w_hi));

        let w_rem = &weights[values.len() - v_rem.len()..];
        for (&v, &w) in v_rem.iter().zip(w_rem.iter()) {
            total_vw += f64::from(v) * f64::from(w);
            total_w += f64::from(w);
        }

        if total_w > f64::from(f32::EPSILON) {
            (total_vw / total_w) as f32
        } else {
            0.0
        }
    }
}
