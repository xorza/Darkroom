//! NEON vectorized exp(x) for 4 f32 values (aarch64).

#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::aarch64::*;

use super::{
    EXP_C1, EXP_C2, EXP_C3, EXP_C4, EXP_C5, EXP_C6, EXP_HI, EXP_LO, LN2_HI, LN2_LO, LOG2E,
};

/// Compute exp(x) for 4 f32 values using NEON.
///
/// Maximum relative error < 2 ULP (~2.4e-7).
#[inline]
pub unsafe fn fast_exp_4_neon(x: &[f32; 4]) -> [f32; 4] {
    let vx = vld1q_f32(x.as_ptr());

    // Clamp input
    let vx = vmaxq_f32(vx, vdupq_n_f32(EXP_LO));
    let vx = vminq_f32(vx, vdupq_n_f32(EXP_HI));

    // Range reduction: n = round(x / ln2)
    let vn_f = vrndmq_f32(vfmaq_f32(vdupq_n_f32(0.5), vx, vdupq_n_f32(LOG2E)));

    // r = x - n * ln2 (hi/lo split)
    let vr = vsubq_f32(vx, vmulq_f32(vn_f, vdupq_n_f32(LN2_HI)));
    let vr = vsubq_f32(vr, vmulq_f32(vn_f, vdupq_n_f32(LN2_LO)));

    // Horner polynomial with FMA
    let vp = vdupq_n_f32(EXP_C6);
    let vp = vfmaq_f32(vdupq_n_f32(EXP_C5), vp, vr);
    let vp = vfmaq_f32(vdupq_n_f32(EXP_C4), vp, vr);
    let vp = vfmaq_f32(vdupq_n_f32(EXP_C3), vp, vr);
    let vp = vfmaq_f32(vdupq_n_f32(EXP_C2), vp, vr);
    let vp = vfmaq_f32(vdupq_n_f32(EXP_C1), vp, vr);
    let vp = vfmaq_f32(vdupq_n_f32(1.0), vp, vr);

    // 2^n via exponent manipulation
    let vn_i = vcvtq_s32_f32(vn_f);
    let vpow2n_bits = vshlq_n_s32::<23>(vaddq_s32(vdupq_n_s32(127), vn_i));
    let vpow2n = vreinterpretq_f32_s32(vpow2n_bits);

    let result = vmulq_f32(vp, vpow2n);
    let mut out = [0.0f32; 4];
    vst1q_f32(out.as_mut_ptr(), result);
    out
}
