//! AVX2 color-preserving arcsinh stretch. The default `auto_asinh` stretch spends ~30% of its time
//! in a per-pixel libm `asinhf` (one call per pixel on the combined intensity). This vectorizes the
//! whole color-preserving pixel op — intensity, `asinh` curve, channel scale, highlight cap — eight
//! pixels at a time, in place, with `asinh(x) = logf(x + √(x²+1))` over a Cephes single-precision
//! `logf` (≈1–2 ULP, i.e. f32-exact). The three channels are contiguous planes, so each iteration is
//! three `loadu` and three `storeu` — on interleaved storage this needed three stride-3
//! `_mm256_i32gather_ps` on load and a 24-store scalar loop on the way back, which against ~35
//! vector ops of actual maths was a large fraction of the kernel.

use std::arch::x86_64::*;

use crate::image_ops::rgb::Rgb;

use crate::image_ops::stretching::simd::{
    LOG_P0, LOG_P1, LOG_P2, LOG_P3, LOG_P4, LOG_P5, LOG_P6, LOG_P7, LOG_P8, LOG_Q1, LOG_Q2, SQRTHF,
};
use crate::image_ops::stretching::{AsinhCurve, color_preserve_pixel};

/// Vectorized single-precision `logf` for 8 lanes (Cephes). Valid for `x > 0`; callers here only
/// ever pass `x = arg + √(arg²+1) ≥ 1`.
#[target_feature(enable = "avx2,fma")]
#[inline]
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
unsafe fn asinh_avx2(x: __m256) -> __m256 {
    let root = _mm256_sqrt_ps(_mm256_fmadd_ps(x, x, _mm256_set1_ps(1.0)));
    // SAFETY: `logf_avx2` needs AVX2+FMA, which this function's own `target_feature` establishes.
    unsafe { logf_avx2(_mm256_add_ps(x, root)) }
}

/// Color-preserving arcsinh stretch of one band of three RGB-f32 **planes**, in place. The three
/// slices must be the same length. Eight pixels per AVX2 iteration; a scalar tail finishes the
/// remainder.
///
/// # Safety
/// The caller must ensure AVX2+FMA are available (checked once at dispatch).
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn asinh_color_preserve_avx2(
    red: &mut [f32],
    green: &mut [f32],
    blue: &mut [f32],
    inv_beta: f32,
    inv_norm: f32,
) {
    // SAFETY: every operation below needs the ISA this function's
    // `target_feature` establishes, and nothing else.
    unsafe {
        debug_assert_eq!(red.len(), green.len());
        debug_assert_eq!(green.len(), blue.len());
        let n_px = red.len();
        let third = _mm256_set1_ps(1.0 / 3.0);
        let vib = _mm256_set1_ps(inv_beta);
        let vin = _mm256_set1_ps(inv_norm);
        let zero = _mm256_setzero_ps();
        let one = _mm256_set1_ps(1.0);

        let mut p = 0;
        while p + 8 <= n_px {
            let r = _mm256_loadu_ps(red.as_ptr().add(p));
            let g = _mm256_loadu_ps(green.as_ptr().add(p));
            let b = _mm256_loadu_ps(blue.as_ptr().add(p));

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
            _mm256_storeu_ps(red.as_mut_ptr().add(p), _mm256_mul_ps(nr, cap));
            _mm256_storeu_ps(green.as_mut_ptr().add(p), _mm256_mul_ps(ng, cap));
            _mm256_storeu_ps(blue.as_mut_ptr().add(p), _mm256_mul_ps(nb, cap));
            p += 8;
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
