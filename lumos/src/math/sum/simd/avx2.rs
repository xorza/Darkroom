//! AVX2 SIMD implementations of sum operations (x86_64).

use std::arch::x86_64::*;

/// Sum f32 values using AVX2 SIMD, accumulating in f64.
///
/// Widening each lane costs one `cvtps_pd` per half and buys the same result
/// [`crate::math::sum::scalar::sum_f32`] produces — the f64 sum rounded once — where compensating
/// in f32 only approached it. See the note on [`crate::math::sum::simd::sum_f32`] for why the
/// wider accumulator is both the accurate and the fast choice.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn sum_f32(values: &[f32]) -> f32 {
    unsafe {
        let mut sum_lo = _mm256_setzero_pd();
        let mut sum_hi = _mm256_setzero_pd();

        let chunks = values.chunks_exact(8);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let v = _mm256_loadu_ps(chunk.as_ptr());
            sum_lo = _mm256_add_pd(sum_lo, _mm256_cvtps_pd(_mm256_castps256_ps128(v)));
            sum_hi = _mm256_add_pd(sum_hi, _mm256_cvtps_pd(_mm256_extractf128_ps::<1>(v)));
        }

        let mut total = reduce_add_pd(_mm256_add_pd(sum_lo, sum_hi));

        for &v in remainder {
            total += f64::from(v);
        }

        total as f32
    }
}

/// Horizontal sum of four f64 lanes, pairing the lanes so the two halves stay independent.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn reduce_add_pd(v: __m256d) -> f64 {
    // SAFETY: every operation below needs the ISA this function's
    // `target_feature` establishes, and nothing else.
    unsafe {
        let mut lanes = [0.0f64; 4];
        _mm256_storeu_pd(lanes.as_mut_ptr(), v);
        (lanes[0] + lanes[1]) + (lanes[2] + lanes[3])
    }
}

/// Weighted mean using AVX2 SIMD, accumulating both sums in f64.
///
/// Widening each lane to f64 rather than compensating in f32 is what keeps this agreeing with
/// [`crate::math::sum::mean_f32`] on the same pixel — see the module note on
/// [`crate::math::sum::simd::weighted_mean_f32`]. Both conversions are lossless and the f64
/// product of two f32s is exact, so this accumulates the same terms as
/// [`crate::math::sum::scalar::weighted_mean_f32`], only in a different order.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    unsafe {
        let mut sum_vw_lo = _mm256_setzero_pd();
        let mut sum_vw_hi = _mm256_setzero_pd();
        let mut sum_w_lo = _mm256_setzero_pd();
        let mut sum_w_hi = _mm256_setzero_pd();

        let v_chunks = values.chunks_exact(8);
        let v_rem = v_chunks.remainder();
        let mut w_ptr = weights.as_ptr();

        for v_chunk in v_chunks {
            let v = _mm256_loadu_ps(v_chunk.as_ptr());
            let w = _mm256_loadu_ps(w_ptr);
            w_ptr = w_ptr.add(8);

            let v_lo = _mm256_cvtps_pd(_mm256_castps256_ps128(v));
            let v_hi = _mm256_cvtps_pd(_mm256_extractf128_ps::<1>(v));
            let w_lo = _mm256_cvtps_pd(_mm256_castps256_ps128(w));
            let w_hi = _mm256_cvtps_pd(_mm256_extractf128_ps::<1>(w));

            sum_vw_lo = _mm256_add_pd(sum_vw_lo, _mm256_mul_pd(v_lo, w_lo));
            sum_vw_hi = _mm256_add_pd(sum_vw_hi, _mm256_mul_pd(v_hi, w_hi));
            sum_w_lo = _mm256_add_pd(sum_w_lo, w_lo);
            sum_w_hi = _mm256_add_pd(sum_w_hi, w_hi);
        }

        let mut total_vw = reduce_add_pd(_mm256_add_pd(sum_vw_lo, sum_vw_hi));
        let mut total_w = reduce_add_pd(_mm256_add_pd(sum_w_lo, sum_w_hi));

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
