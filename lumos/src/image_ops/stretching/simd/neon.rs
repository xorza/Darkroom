//! NEON arcsinh stretch (aarch64), four samples at a time: the plane curve, and the whole
//! color-preserving pixel op — intensity, curve, channel scale, highlight cap. NEON is mandatory on
//! aarch64, so the caller dispatches on `cfg(target_arch)` with no runtime feature check.

use std::arch::aarch64::*;

use crate::image_ops::rgb::Rgb;

use crate::image_ops::stretching::simd::{
    ASINH_LOG_FROM, LOG_P0, LOG_P1, LOG_P2, LOG_P3, LOG_P4, LOG_P5, LOG_P6, LOG_P7, LOG_P8, LOG_Q1,
    LOG_Q2, SQRTHF,
};
use crate::image_ops::stretching::{AsinhCurve, ToneCurve, color_preserve_pixel};
use std::f32::consts::LN_2;

/// Vectorized single-precision `logf` for 4 lanes (Cephes), ~1 ULP. Valid for `x > 0`; other lanes
/// give a value [`asinh_neon`] discards.
#[inline]
unsafe fn logf_neon(x: float32x4_t) -> float32x4_t {
    unsafe {
        // frexp: split x = m · 2^e with the mantissa m in [0.5, 1). For x > 0 the sign bit is
        // clear, making the arithmetic shift of the exponent field equivalent to a logical one.
        let xi = vreinterpretq_s32_f32(x);
        let e = vcvtq_f32_s32(vsubq_s32(vshrq_n_s32::<23>(xi), vdupq_n_s32(126)));
        let m = vreinterpretq_f32_s32(vorrq_s32(
            vandq_s32(xi, vdupq_n_s32(0x807f_ffffu32 as i32)),
            vdupq_n_s32(0x3f00_0000),
        ));

        // Bring m into [-0.293, 0.414]: if m < √½, use 2m−1 and drop the exponent by one; else m−1.
        let one = vdupq_n_f32(1.0);
        let zero = vdupq_n_f32(0.0);
        let lt = vcltq_f32(m, vdupq_n_f32(SQRTHF));
        let e = vsubq_f32(e, vbslq_f32(lt, one, zero));
        let m = vaddq_f32(vsubq_f32(m, one), vbslq_f32(lt, m, zero));

        let z = vmulq_f32(m, m);
        // Horner: vfmaq_f32(c, a, b) = c + a·b.
        let mut y = vdupq_n_f32(LOG_P0);
        y = vfmaq_f32(vdupq_n_f32(LOG_P1), y, m);
        y = vfmaq_f32(vdupq_n_f32(LOG_P2), y, m);
        y = vfmaq_f32(vdupq_n_f32(LOG_P3), y, m);
        y = vfmaq_f32(vdupq_n_f32(LOG_P4), y, m);
        y = vfmaq_f32(vdupq_n_f32(LOG_P5), y, m);
        y = vfmaq_f32(vdupq_n_f32(LOG_P6), y, m);
        y = vfmaq_f32(vdupq_n_f32(LOG_P7), y, m);
        y = vfmaq_f32(vdupq_n_f32(LOG_P8), y, m);
        y = vmulq_f32(vmulq_f32(y, m), z);

        y = vfmaq_f32(y, e, vdupq_n_f32(LOG_Q1)); // + e·ln2_lo
        y = vfmsq_f32(y, vdupq_n_f32(0.5), z); // − z/2
        let res = vaddq_f32(m, y);
        vfmaq_f32(res, e, vdupq_n_f32(LOG_Q2)) // + e·ln2_hi
    }
}

/// Vectorized `asinh(x)` for `x ≥ 0`, to a few ULP relative at every magnitude — the steps the
/// tests' `asinh_pos_scalar` spells out one lane at a time. Negative `x` gives a non-positive value
/// or NaN, both of which the callers' clamp to `[0, 1]` turns to 0.
#[inline]
unsafe fn asinh_neon(x: float32x4_t) -> float32x4_t {
    unsafe {
        let one = vdupq_n_f32(1.0);
        let s = vmulq_f32(x, x);
        let u = vaddq_f32(
            x,
            vdivq_f32(s, vaddq_f32(one, vsqrtq_f32(vaddq_f32(one, s)))),
        );
        let w = vaddq_f32(one, u);
        let dw = vsubq_f32(w, one);
        // One `logf` serves both forms: of `1 + u` below the switch, of `x` past it.
        let large = vcgtq_f32(x, vdupq_n_f32(ASINH_LOG_FROM));
        let log = logf_neon(vbslq_f32(large, x, w));
        let log1p = vbslq_f32(
            vceqq_f32(dw, vdupq_n_f32(0.0)),
            u,
            vmulq_f32(log, vdivq_f32(u, dw)),
        );
        vbslq_f32(large, vaddq_f32(log, vdupq_n_f32(LN_2)), log1p)
    }
}

/// The arcsinh plane curve over one band, in place: `clamp(asinh(v / β) / norm, 0, 1)`. Four
/// samples per iteration; [`AsinhCurve::eval`] finishes the tail.
///
/// # Safety
/// Caller must be on aarch64 (NEON is always available there).
pub(super) unsafe fn asinh_plane_neon(plane: &mut [f32], inv_beta: f32, inv_norm: f32) {
    unsafe {
        let (vib, vin) = (vdupq_n_f32(inv_beta), vdupq_n_f32(inv_norm));
        let (zero, one) = (vdupq_n_f32(0.0), vdupq_n_f32(1.0));
        let mut p = 0;
        while p + 4 <= plane.len() {
            let v = vld1q_f32(plane.as_ptr().add(p));
            let curved = vmulq_f32(asinh_neon(vmulq_f32(v, vib)), vin);
            // The `nm` forms return the number for a NaN lane: it becomes black.
            let out = vminnmq_f32(vmaxnmq_f32(curved, zero), one);
            vst1q_f32(plane.as_mut_ptr().add(p), out);
            p += 4;
        }
        let curve = AsinhCurve { inv_beta, inv_norm };
        for value in &mut plane[p..] {
            *value = curve.eval(*value);
        }
    }
}

/// Color-preserving arcsinh stretch of one band of three RGB-f32 **planes**, in place. The three
/// slices must be the same length. Four pixels per NEON iteration; a scalar tail finishes the
/// remainder.
///
/// # Safety
/// Caller must be on aarch64 (NEON is always available there).
pub(super) unsafe fn asinh_color_preserve_neon(
    red: &mut [f32],
    green: &mut [f32],
    blue: &mut [f32],
    inv_beta: f32,
    inv_norm: f32,
) {
    unsafe {
        debug_assert_eq!(red.len(), green.len());
        debug_assert_eq!(green.len(), blue.len());
        let n_px = red.len();
        let third = vdupq_n_f32(1.0 / 3.0);
        let vib = vdupq_n_f32(inv_beta);
        let vin = vdupq_n_f32(inv_norm);
        let zero = vdupq_n_f32(0.0);
        let one = vdupq_n_f32(1.0);

        let mut p = 0;
        while p + 4 <= n_px {
            let r = vld1q_f32(red.as_ptr().add(p));
            let g = vld1q_f32(green.as_ptr().add(p));
            let b = vld1q_f32(blue.as_ptr().add(p));

            let intensity = vmulq_f32(vaddq_f32(vaddq_f32(r, g), b), third);
            let curved = asinh_neon(vmulq_f32(intensity, vib));
            let e = vminnmq_f32(vmaxnmq_f32(vmulq_f32(curved, vin), zero), one);
            // scale = eval/intensity where intensity > 0, else 0 (sub-background pixels → black).
            let pos = vcgtq_f32(intensity, zero);
            let scale = vbslq_f32(pos, vdivq_f32(e, intensity), zero);

            let nr = vmulq_f32(r, scale);
            let ng = vmulq_f32(g, scale);
            let nb = vmulq_f32(b, scale);
            // Hue-preserving highlight cap: divide by the max channel when it exceeds 1.
            let maxc = vmaxq_f32(vmaxq_f32(nr, ng), nb);
            let cap = vbslq_f32(vcgtq_f32(maxc, one), vdivq_f32(one, maxc), one);

            // A channel below black, possible beside a positive intensity, clamps to 0.
            let out = |v| vmaxq_f32(vmulq_f32(v, cap), zero);
            vst1q_f32(red.as_mut_ptr().add(p), out(nr));
            vst1q_f32(green.as_mut_ptr().add(p), out(ng));
            vst1q_f32(blue.as_mut_ptr().add(p), out(nb));
            p += 4;
        }

        let curve = AsinhCurve { inv_beta, inv_norm };
        while p < n_px {
            let out = color_preserve_pixel(
                Rgb {
                    r: red[p],
                    g: green[p],
                    b: blue[p],
                },
                &curve,
            );
            red[p] = out.r;
            green[p] = out.g;
            blue[p] = out.b;
            p += 1;
        }
    }
}
