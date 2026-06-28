//! AVX2 color-preserving arcsinh stretch. The default `auto_asinh` stretch spends ~30% of its time
//! in a per-pixel libm `asinhf` (one call per pixel on the combined intensity). This vectorizes the
//! whole color-preserving pixel op — intensity, `asinh` curve, channel scale, highlight cap — eight
//! pixels at a time, in place, with `asinh(x) = logf(x + √(x²+1))` over a Cephes single-precision
//! `logf` (≈1–2 ULP, i.e. f32-exact). The three channels are gathered with a constant stride-3 index
//! (no hand-rolled deinterleave) and the result scalar-written back.

use std::arch::x86_64::*;

use common::Rgb;

use crate::image_ops::stretching::{AsinhCurve, color_preserve_pixel};

// Cephes single-precision logf polynomial (cephes/logf.c), accurate to ~1 ULP on the reduced
// mantissa. `Q1`/`Q2` are the two-part ln(2) used to reassemble log from mantissa + exponent.
const LOG_P0: f32 = 7.037_683_6e-2;
const LOG_P1: f32 = -1.151_461e-1;
const LOG_P2: f32 = 1.167_699_9e-1;
const LOG_P3: f32 = -1.242_014_1e-1;
const LOG_P4: f32 = 1.424_932_3e-1;
const LOG_P5: f32 = -1.666_805_8e-1;
const LOG_P6: f32 = 2.000_071_5e-1;
const LOG_P7: f32 = -2.499_999_4e-1;
const LOG_P8: f32 = 3.333_333e-1;
const SQRTHF: f32 = 0.707_106_77;
const LOG_Q1: f32 = -2.121_944_4e-4;
const LOG_Q2: f32 = 0.693_359_4;

/// Vectorized single-precision `logf` for 8 lanes (Cephes). Valid for `x > 0`; callers here only
/// ever pass `x = arg + √(arg²+1) ≥ 1`.
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn logf_avx2(x: __m256) -> __m256 {
    // frexp: split x = m · 2^e with the mantissa m in [0.5, 1).
    let xi = _mm256_castps_si256(x);
    let e = _mm256_cvtepi32_ps(_mm256_sub_epi32(
        _mm256_srli_epi32(xi, 23),
        _mm256_set1_epi32(126),
    ));
    let m = _mm256_castsi256_ps(_mm256_or_si256(
        _mm256_and_si256(xi, _mm256_set1_epi32(0x807f_ffffu32 as i32)),
        _mm256_set1_epi32(0x3f00_0000),
    ));

    // Bring m into [-0.293, 0.414]: if m < √½, use 2m−1 and drop the exponent by one; else m−1.
    let lt = _mm256_cmp_ps::<_CMP_LT_OQ>(m, _mm256_set1_ps(SQRTHF));
    let e = _mm256_sub_ps(e, _mm256_and_ps(lt, _mm256_set1_ps(1.0)));
    let m = _mm256_add_ps(_mm256_sub_ps(m, _mm256_set1_ps(1.0)), _mm256_and_ps(lt, m));

    let z = _mm256_mul_ps(m, m);
    let mut y = _mm256_set1_ps(LOG_P0);
    y = _mm256_fmadd_ps(y, m, _mm256_set1_ps(LOG_P1));
    y = _mm256_fmadd_ps(y, m, _mm256_set1_ps(LOG_P2));
    y = _mm256_fmadd_ps(y, m, _mm256_set1_ps(LOG_P3));
    y = _mm256_fmadd_ps(y, m, _mm256_set1_ps(LOG_P4));
    y = _mm256_fmadd_ps(y, m, _mm256_set1_ps(LOG_P5));
    y = _mm256_fmadd_ps(y, m, _mm256_set1_ps(LOG_P6));
    y = _mm256_fmadd_ps(y, m, _mm256_set1_ps(LOG_P7));
    y = _mm256_fmadd_ps(y, m, _mm256_set1_ps(LOG_P8));
    y = _mm256_mul_ps(_mm256_mul_ps(y, m), z);

    y = _mm256_fmadd_ps(e, _mm256_set1_ps(LOG_Q1), y); // + e·ln2_lo
    y = _mm256_fnmadd_ps(_mm256_set1_ps(0.5), z, y); // − z/2
    let res = _mm256_add_ps(m, y);
    _mm256_fmadd_ps(e, _mm256_set1_ps(LOG_Q2), res) // + e·ln2_hi
}

/// Vectorized `asinh(x) = logf(x + √(x²+1))`, exact for all real x (the argument to logf is always
/// positive).
#[target_feature(enable = "avx2,fma")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn asinh_avx2(x: __m256) -> __m256 {
    let root = _mm256_sqrt_ps(_mm256_fmadd_ps(x, x, _mm256_set1_ps(1.0)));
    logf_avx2(_mm256_add_ps(x, root))
}

/// Color-preserving arcsinh stretch of an interleaved RGB-f32 `block` (length a multiple of 3) in
/// place. Eight pixels per AVX2 iteration; a scalar tail finishes the remainder.
///
/// # Safety
/// The caller must ensure AVX2+FMA are available (checked once at dispatch).
#[target_feature(enable = "avx2,fma")]
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn asinh_color_preserve_avx2(block: &mut [f32], inv_beta: f32, inv_norm: f32) {
    let n_px = block.len() / 3;
    let base = block.as_ptr();
    let idx = _mm256_setr_epi32(0, 3, 6, 9, 12, 15, 18, 21); // stride-3 pixel offsets
    let third = _mm256_set1_ps(1.0 / 3.0);
    let vib = _mm256_set1_ps(inv_beta);
    let vin = _mm256_set1_ps(inv_norm);
    let zero = _mm256_setzero_ps();
    let one = _mm256_set1_ps(1.0);

    let mut p = 0;
    while p + 8 <= n_px {
        let g0 = base.add(p * 3);
        let r = _mm256_i32gather_ps::<4>(g0, idx);
        let g = _mm256_i32gather_ps::<4>(g0.add(1), idx);
        let b = _mm256_i32gather_ps::<4>(g0.add(2), idx);

        let intensity = _mm256_mul_ps(_mm256_add_ps(_mm256_add_ps(r, g), b), third);
        let curved = asinh_avx2(_mm256_mul_ps(intensity, vib));
        let e = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(curved, vin), zero), one);
        // scale = eval/intensity where intensity > 0, else 0 (sub-background pixels → black).
        let pos = _mm256_cmp_ps::<_CMP_GT_OQ>(intensity, zero);
        let scale = _mm256_and_ps(pos, _mm256_div_ps(e, intensity));

        let nr = _mm256_mul_ps(r, scale);
        let ng = _mm256_mul_ps(g, scale);
        let nb = _mm256_mul_ps(b, scale);
        // Hue-preserving highlight cap: divide by the max channel when it exceeds 1.
        let maxc = _mm256_max_ps(_mm256_max_ps(nr, ng), nb);
        let cap = _mm256_blendv_ps(
            one,
            _mm256_div_ps(one, maxc),
            _mm256_cmp_ps::<_CMP_GT_OQ>(maxc, one),
        );
        let (mut tr, mut tg, mut tb) = ([0f32; 8], [0f32; 8], [0f32; 8]);
        _mm256_storeu_ps(tr.as_mut_ptr(), _mm256_mul_ps(nr, cap));
        _mm256_storeu_ps(tg.as_mut_ptr(), _mm256_mul_ps(ng, cap));
        _mm256_storeu_ps(tb.as_mut_ptr(), _mm256_mul_ps(nb, cap));
        for k in 0..8 {
            let o = (p + k) * 3;
            *block.get_unchecked_mut(o) = tr[k];
            *block.get_unchecked_mut(o + 1) = tg[k];
            *block.get_unchecked_mut(o + 2) = tb[k];
        }
        p += 8;
    }

    let curve = AsinhCurve { inv_beta, inv_norm };
    while p < n_px {
        let o = p * 3;
        let out = color_preserve_pixel(
            Rgb {
                r: block[o],
                g: block[o + 1],
                b: block[o + 2],
            },
            &curve,
        );
        block[o] = out.r;
        block[o + 1] = out.g;
        block[o + 2] = out.b;
        p += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avx2_matches_scalar_reference() {
        if !common::cpu_features::has_avx2() {
            return;
        }
        let beta = 0.05f32;
        let inv_beta = 1.0 / beta;
        let inv_norm = 1.0 / inv_beta.asinh();

        // 19 pixels (not a multiple of 8 → exercises the SIMD body and the scalar tail), spanning
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
        let mut simd: Vec<f32> = pixels.iter().flatten().copied().collect();
        unsafe { asinh_color_preserve_avx2(&mut simd, inv_beta, inv_norm) };

        // Reference: the production scalar path (`color_preserve_pixel` ∘ `AsinhCurve`), so the SIMD
        // body is pinned to exactly what the non-AVX2 path produces.
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
            let got = [simd[i * 3], simd[i * 3 + 1], simd[i * 3 + 2]];
            for (g, e) in got.iter().zip([exp.r, exp.g, exp.b]) {
                assert!(
                    (g - e).abs() < 1e-5,
                    "pixel {i} {px:?}: simd {g} vs scalar {e}"
                );
            }
        }
    }
}
