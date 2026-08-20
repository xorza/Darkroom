//! AVX2 backends for the sum operations (x86_64).

use std::arch::x86_64::*;

use crate::math::sum::scalar;
use crate::math::sum::weighted_sums::WeightedSums;

/// Horizontal sum of four f64 lanes, pairing them so the two halves stay independent.
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

/// Sum f32 values into an f64 accumulator, unrounded.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn sum_f32(values: &[f32]) -> f64 {
    unsafe {
        let mut sum_lo = _mm256_setzero_pd();
        let mut sum_hi = _mm256_setzero_pd();

        let (chunks, tail) = values.as_chunks::<8>();

        for chunk in chunks {
            let v = _mm256_loadu_ps(chunk.as_ptr());
            sum_lo = _mm256_add_pd(sum_lo, _mm256_cvtps_pd(_mm256_castps256_ps128(v)));
            sum_hi = _mm256_add_pd(sum_hi, _mm256_cvtps_pd(_mm256_extractf128_ps::<1>(v)));
        }

        reduce_add_pd(_mm256_add_pd(sum_lo, sum_hi)) + scalar::sum_f32(tail)
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
/// Caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
pub(super) unsafe fn weighted_sums(values: &[f32], weights: &[f32]) -> WeightedSums {
    unsafe {
        let mut weighted_lo = _mm256_setzero_pd();
        let mut weighted_hi = _mm256_setzero_pd();
        let mut weight_lo = _mm256_setzero_pd();
        let mut weight_hi = _mm256_setzero_pd();

        let (value_chunks, value_tail) = values.as_chunks::<8>();
        let (weight_chunks, weight_tail) = weights.as_chunks::<8>();

        for (value_chunk, weight_chunk) in value_chunks.iter().zip(weight_chunks) {
            let v = _mm256_loadu_ps(value_chunk.as_ptr());
            let w = _mm256_loadu_ps(weight_chunk.as_ptr());

            let v_lo = _mm256_cvtps_pd(_mm256_castps256_ps128(v));
            let v_hi = _mm256_cvtps_pd(_mm256_extractf128_ps::<1>(v));
            let w_lo = _mm256_cvtps_pd(_mm256_castps256_ps128(w));
            let w_hi = _mm256_cvtps_pd(_mm256_extractf128_ps::<1>(w));

            weighted_lo = _mm256_add_pd(weighted_lo, _mm256_mul_pd(v_lo, w_lo));
            weighted_hi = _mm256_add_pd(weighted_hi, _mm256_mul_pd(v_hi, w_hi));
            weight_lo = _mm256_add_pd(weight_lo, w_lo);
            weight_hi = _mm256_add_pd(weight_hi, w_hi);
        }

        let vector = WeightedSums {
            weighted_values: reduce_add_pd(_mm256_add_pd(weighted_lo, weighted_hi)),
            weight_total: reduce_add_pd(_mm256_add_pd(weight_lo, weight_hi)),
        };

        vector + scalar::weighted_sums(value_tail, weight_tail)
    }
}
