//! NEON color-preserving arcsinh stretch (aarch64). The aarch64 counterpart of
//! [`crate::image_ops::stretching::simd_avx2`]: the default `auto_asinh` stretch spends ~30% of its
//! time in a per-pixel libm `asinhf` (one call per pixel on the combined intensity). This vectorizes
//! the whole color-preserving pixel op — intensity, `asinh` curve, channel scale, highlight cap —
//! four pixels at a time, in place, with `asinh(x) = logf(x + √(x²+1))` over a Cephes single-precision
//! `logf` (≈1–2 ULP, i.e. f32-exact). The channels are contiguous planes, so loads and stores are
//! plain `vld1q_f32`/`vst1q_f32`. Note this is the one place planar storage bought aarch64 nothing:
//! on interleaved data `vld3q_f32`/`vst3q_f32` deinterleaved in hardware, one instruction each,
//! where AVX2 needed three gathers and a 24-store scalar loop. NEON is mandatory on aarch64, so the
//! caller dispatches on `cfg(target_arch)` with no runtime feature check.

use std::arch::aarch64::*;

use crate::image_ops::rgb::Rgb;

use crate::image_ops::stretching::{
    AsinhCurve, LOG_P0, LOG_P1, LOG_P2, LOG_P3, LOG_P4, LOG_P5, LOG_P6, LOG_P7, LOG_P8, LOG_Q1,
    LOG_Q2, SQRTHF, color_preserve_pixel,
};

/// Vectorized single-precision `logf` for 4 lanes (Cephes). Valid for `x > 0`; callers here only
/// ever pass `x = arg + √(arg²+1) ≥ 1`.
#[inline]
unsafe fn logf_neon(x: float32x4_t) -> float32x4_t {
    unsafe {
        // frexp: split x = m · 2^e with the mantissa m in [0.5, 1). x ≥ 1 so the sign bit is clear,
        // making the arithmetic shift of the exponent field equivalent to a logical one.
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

/// Vectorized `asinh(x) = logf(x + √(x²+1))`, exact for all real x (the argument to logf is always
/// positive).
#[inline]
unsafe fn asinh_neon(x: float32x4_t) -> float32x4_t {
    unsafe {
        let root = vsqrtq_f32(vfmaq_f32(vdupq_n_f32(1.0), x, x));
        logf_neon(vaddq_f32(x, root))
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
            let e = vminq_f32(vmaxq_f32(vmulq_f32(curved, vin), zero), one);
            // scale = eval/intensity where intensity > 0, else 0 (sub-background pixels → black).
            let pos = vcgtq_f32(intensity, zero);
            let scale = vbslq_f32(pos, vdivq_f32(e, intensity), zero);

            let nr = vmulq_f32(r, scale);
            let ng = vmulq_f32(g, scale);
            let nb = vmulq_f32(b, scale);
            // Hue-preserving highlight cap: divide by the max channel when it exceeds 1.
            let maxc = vmaxq_f32(vmaxq_f32(nr, ng), nb);
            let cap = vbslq_f32(vcgtq_f32(maxc, one), vdivq_f32(one, maxc), one);

            vst1q_f32(red.as_mut_ptr().add(p), vmulq_f32(nr, cap));
            vst1q_f32(green.as_mut_ptr().add(p), vmulq_f32(ng, cap));
            vst1q_f32(blue.as_mut_ptr().add(p), vmulq_f32(nb, cap));
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

#[cfg(test)]
mod tests {
    use crate::image_ops::stretching::simd_neon::*;

    #[test]
    fn neon_matches_scalar_reference() {
        let beta = 0.05f32;
        let inv_beta = 1.0 / beta;
        let inv_norm = 1.0 / inv_beta.asinh();

        // 19 pixels per plane (not a multiple of 4 → exercises the SIMD body and the scalar tail), spanning
        // background, midtones, above-unity stars, exact zero, a tiny value, and a sub-background
        // pixel whose channels sum to ≤ 0 (must map to black).
        let pixels: Vec<[f32; 3]> = vec![
            [0.02, 0.018, 0.021],
            [0.05, 0.04, 0.045],
            [0.2, 0.1, 0.1],
            [0.9, 0.45, 0.45],
            [3.0, 2.0, 1.0],
            [1.5, 1.5, 1.5],
            [0.0, 0.0, 0.0],
            [1e-5, 1e-5, 1e-5],
            [0.3, 0.0, 0.0],
            [-0.05, -0.05, -0.05],
            [0.12, 0.34, 0.07],
            [0.01, 0.5, 0.9],
            [5.0, 0.01, 0.01],
            [0.04, 0.04, 0.04],
            [0.6, 0.6, 0.59],
            [0.15, 0.15, 0.16],
            [0.08, 0.02, 0.5],
            [2.5, 2.4, 2.6],
            [0.07, 0.06, 0.08],
        ];
        // Planes, not interleaved samples — the kernel now takes one slice per channel.
        let mut r: Vec<f32> = pixels.iter().map(|px| px[0]).collect();
        let mut g: Vec<f32> = pixels.iter().map(|px| px[1]).collect();
        let mut b: Vec<f32> = pixels.iter().map(|px| px[2]).collect();
        unsafe { asinh_color_preserve_neon(&mut r, &mut g, &mut b, inv_beta, inv_norm) };

        // Reference: the production scalar path (`color_preserve_pixel` ∘ `AsinhCurve`), so the SIMD
        // body is pinned to exactly what the non-NEON path produces.
        let curve = AsinhCurve { inv_beta, inv_norm };
        for (i, px) in pixels.iter().enumerate() {
            let exp = color_preserve_pixel(
                Rgb {
                    r: px[0],
                    g: px[1],
                    b: px[2],
                },
                &curve,
            );
            let got = [r[i], g[i], b[i]];
            for (g, e) in got.iter().zip([exp.r, exp.g, exp.b]) {
                assert!(
                    (g - e).abs() < 1e-5,
                    "pixel {i} {px:?}: simd {g} vs scalar {e}"
                );
            }
        }
    }
}
