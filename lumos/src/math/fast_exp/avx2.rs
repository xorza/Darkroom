//! AVX2 vectorized exp(x) for 8 f32 values.

#![allow(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]

use std::arch::x86_64::*;

use super::{
    EXP_C1, EXP_C2, EXP_C3, EXP_C4, EXP_C5, EXP_C6, EXP_HI, EXP_LO, LN2_HI, LN2_LO, LOG2E,
};

/// Compute exp(x) for 8 f32 values using AVX2+FMA.
///
/// Maximum relative error < 2 ULP (~2.4e-7).
/// Processes all 8 values entirely in SIMD registers.
#[inline]
#[target_feature(enable = "avx2", enable = "fma")]
pub unsafe fn fast_exp_8_avx2(x: &[f32; 8]) -> [f32; 8] {
    let vx = _mm256_loadu_ps(x.as_ptr());
    let result = fast_exp_8_avx2_m256(vx);
    let mut out = [0.0f32; 8];
    _mm256_storeu_ps(out.as_mut_ptr(), result);
    out
}

/// Compute exp(x) for an __m256 register using AVX2+FMA.
#[inline]
#[target_feature(enable = "avx2", enable = "fma")]
pub unsafe fn fast_exp_8_avx2_m256(vx: __m256) -> __m256 {
    // Clamp input to avoid overflow/underflow
    let vlo = _mm256_set1_ps(EXP_LO);
    let vhi = _mm256_set1_ps(EXP_HI);
    let vx = _mm256_max_ps(vx, vlo);
    let vx = _mm256_min_ps(vx, vhi);

    // Range reduction: n = round(x / ln2)
    let vlog2e = _mm256_set1_ps(LOG2E);
    let vhalf = _mm256_set1_ps(0.5);
    // n = floor(x * log2e + 0.5)
    let vn = _mm256_floor_ps(_mm256_fmadd_ps(vx, vlog2e, vhalf));

    // r = x - n * ln2  (using hi/lo split for precision)
    let vln2_hi = _mm256_set1_ps(LN2_HI);
    let vln2_lo = _mm256_set1_ps(LN2_LO);
    let vr = _mm256_sub_ps(vx, _mm256_mul_ps(vn, vln2_hi));
    let vr = _mm256_sub_ps(vr, _mm256_mul_ps(vn, vln2_lo));

    // Polynomial evaluation using Horner's method with FMA:
    // p = 1 + r·(c1 + r·(c2 + r·(c3 + r·(c4 + r·(c5 + r·c6)))))
    let vc6 = _mm256_set1_ps(EXP_C6);
    let vc5 = _mm256_set1_ps(EXP_C5);
    let vc4 = _mm256_set1_ps(EXP_C4);
    let vc3 = _mm256_set1_ps(EXP_C3);
    let vc2 = _mm256_set1_ps(EXP_C2);
    let vc1 = _mm256_set1_ps(EXP_C1);
    let vone = _mm256_set1_ps(1.0);

    let vp = vc6;
    let vp = _mm256_fmadd_ps(vp, vr, vc5);
    let vp = _mm256_fmadd_ps(vp, vr, vc4);
    let vp = _mm256_fmadd_ps(vp, vr, vc3);
    let vp = _mm256_fmadd_ps(vp, vr, vc2);
    let vp = _mm256_fmadd_ps(vp, vr, vc1);
    let vp = _mm256_fmadd_ps(vp, vr, vone);

    // Multiply by 2^n: add n to the IEEE 754 exponent bits
    // 2^n = float_from_bits((127 + n) << 23)
    let v127 = _mm256_set1_epi32(127);
    let vn_i = _mm256_cvtps_epi32(vn);
    let vpow2n_bits = _mm256_slli_epi32(_mm256_add_epi32(v127, vn_i), 23);
    let vpow2n = _mm256_castsi256_ps(vpow2n_bits);

    _mm256_mul_ps(vp, vpow2n)
}
