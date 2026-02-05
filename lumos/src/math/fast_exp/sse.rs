//! SSE vectorized exp(x) for 4 f32 values.

#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::x86_64::*;

use super::{
    EXP_C1, EXP_C2, EXP_C3, EXP_C4, EXP_C5, EXP_C6, EXP_HI, EXP_LO, LN2_HI, LN2_LO, LOG2E,
};

/// Compute exp(x) for an __m128 register using SSE4.1.
#[inline]
#[target_feature(enable = "sse4.1")]
pub unsafe fn fast_exp_4_sse_m128(vx: __m128) -> __m128 {
    // Clamp input to avoid overflow/underflow
    let vlo = _mm_set1_ps(EXP_LO);
    let vhi = _mm_set1_ps(EXP_HI);
    let vx = _mm_max_ps(vx, vlo);
    let vx = _mm_min_ps(vx, vhi);

    // Range reduction: n = round(x / ln2)
    let vlog2e = _mm_set1_ps(LOG2E);
    let vhalf = _mm_set1_ps(0.5);
    // n = floor(x * log2e + 0.5)
    let vn = _mm_floor_ps(_mm_add_ps(_mm_mul_ps(vx, vlog2e), vhalf));

    // r = x - n * ln2  (using hi/lo split for precision)
    let vln2_hi = _mm_set1_ps(LN2_HI);
    let vln2_lo = _mm_set1_ps(LN2_LO);
    let vr = _mm_sub_ps(vx, _mm_mul_ps(vn, vln2_hi));
    let vr = _mm_sub_ps(vr, _mm_mul_ps(vn, vln2_lo));

    // Polynomial evaluation using Horner's method:
    // p = 1 + r·(c1 + r·(c2 + r·(c3 + r·(c4 + r·(c5 + r·c6)))))
    let vc6 = _mm_set1_ps(EXP_C6);
    let vc5 = _mm_set1_ps(EXP_C5);
    let vc4 = _mm_set1_ps(EXP_C4);
    let vc3 = _mm_set1_ps(EXP_C3);
    let vc2 = _mm_set1_ps(EXP_C2);
    let vc1 = _mm_set1_ps(EXP_C1);
    let vone = _mm_set1_ps(1.0);

    // SSE doesn't have FMA, use mul+add
    let vp = vc6;
    let vp = _mm_add_ps(_mm_mul_ps(vp, vr), vc5);
    let vp = _mm_add_ps(_mm_mul_ps(vp, vr), vc4);
    let vp = _mm_add_ps(_mm_mul_ps(vp, vr), vc3);
    let vp = _mm_add_ps(_mm_mul_ps(vp, vr), vc2);
    let vp = _mm_add_ps(_mm_mul_ps(vp, vr), vc1);
    let vp = _mm_add_ps(_mm_mul_ps(vp, vr), vone);

    // Multiply by 2^n: add n to the IEEE 754 exponent bits
    let v127 = _mm_set1_epi32(127);
    let vn_i = _mm_cvtps_epi32(vn);
    let vpow2n_bits = _mm_slli_epi32(_mm_add_epi32(v127, vn_i), 23);
    let vpow2n = _mm_castsi128_ps(vpow2n_bits);

    _mm_mul_ps(vp, vpow2n)
}
