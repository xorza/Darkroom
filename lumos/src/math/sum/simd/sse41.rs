//! SSE SIMD weighted mean (x86_64).

use std::arch::x86_64::*;

/// Horizontal sum of two f64 lanes.
#[inline]
#[target_feature(enable = "sse4.1")]
unsafe fn reduce_add_pd(v: __m128d) -> f64 {
    // SAFETY: every operation below needs the ISA this function's
    // `target_feature` establishes, and nothing else.
    unsafe {
        let mut lanes = [0.0f64; 2];
        _mm_storeu_pd(lanes.as_mut_ptr(), v);
        lanes[0] + lanes[1]
    }
}

/// Weighted mean using SSE4.1 SIMD, accumulating both sums in f64.
///
/// Widening each lane to f64 rather than compensating in f32 is what keeps this agreeing with
/// [`crate::math::sum::mean_f32`] on the same pixel — see the module note on
/// [`crate::math::sum::simd::weighted_mean_f32`]. Both conversions are lossless and the f64
/// product of two f32s is exact, so this accumulates the same terms as
/// [`crate::math::sum::scalar::weighted_mean_f32`], only in a different order.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    unsafe {
        let mut sum_vw_lo = _mm_setzero_pd();
        let mut sum_vw_hi = _mm_setzero_pd();
        let mut sum_w_lo = _mm_setzero_pd();
        let mut sum_w_hi = _mm_setzero_pd();

        let v_chunks = values.chunks_exact(4);
        let v_rem = v_chunks.remainder();
        let mut w_ptr = weights.as_ptr();

        for v_chunk in v_chunks {
            let v = _mm_loadu_ps(v_chunk.as_ptr());
            let w = _mm_loadu_ps(w_ptr);
            w_ptr = w_ptr.add(4);

            let v_lo = _mm_cvtps_pd(v);
            let v_hi = _mm_cvtps_pd(_mm_movehl_ps(v, v));
            let w_lo = _mm_cvtps_pd(w);
            let w_hi = _mm_cvtps_pd(_mm_movehl_ps(w, w));

            sum_vw_lo = _mm_add_pd(sum_vw_lo, _mm_mul_pd(v_lo, w_lo));
            sum_vw_hi = _mm_add_pd(sum_vw_hi, _mm_mul_pd(v_hi, w_hi));
            sum_w_lo = _mm_add_pd(sum_w_lo, w_lo);
            sum_w_hi = _mm_add_pd(sum_w_hi, w_hi);
        }

        let mut total_vw = reduce_add_pd(_mm_add_pd(sum_vw_lo, sum_vw_hi));
        let mut total_w = reduce_add_pd(_mm_add_pd(sum_w_lo, sum_w_hi));

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
