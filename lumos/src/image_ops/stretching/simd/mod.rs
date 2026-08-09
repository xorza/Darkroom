//! Vector backends for the color-preserving arcsinh curve, and the dispatch between them.
//!
//! The per-pixel `asinh` is the curve's hot spot, so each backend computes it ≈ f32-exact (~1 ULP
//! vs libm) over a whole band of three planes at once. Planar storage is what lets them take three
//! plain `loadu`/`storeu` per vector: on interleaved data AVX2 needed three stride-3 gathers and a
//! 24-store scalar write-back per 8 pixels, while NEON's `vld3q_f32`/`vst3q_f32` did it in one
//! instruction each — a large win on x86 and roughly neutral on aarch64.

use crate::image_ops::rgb::Rgb;
use crate::image_ops::stretching::{AsinhCurve, color_preserve_pixel};
use crate::simd::dispatch;

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(target_arch = "aarch64")]
mod neon;

/// Cephes single-precision `logf` polynomial coefficients (`cephes/logf.c`), accurate to ~1 ULP on
/// the reduced mantissa. Shared verbatim by the AVX2 and NEON `asinh` backends (`asinh(x) =
/// logf(x + √(x²+1))`) so the two arches stay bit-for-bit identical — one source of truth, no
/// "keep in sync" drift. `Q1`/`Q2` are the two-part ln(2) that reassembles log from mantissa +
/// exponent.
pub(super) const LOG_P0: f32 = 7.037_683_6e-2;
pub(super) const LOG_P1: f32 = -1.151_461e-1;
pub(super) const LOG_P2: f32 = 1.167_699_9e-1;
pub(super) const LOG_P3: f32 = -1.242_014_1e-1;
pub(super) const LOG_P4: f32 = 1.424_932_3e-1;
pub(super) const LOG_P5: f32 = -1.666_805_8e-1;
pub(super) const LOG_P6: f32 = 2.000_071_5e-1;
pub(super) const LOG_P7: f32 = -2.499_999_4e-1;
pub(super) const LOG_P8: f32 = 3.333_333e-1;
pub(super) const SQRTHF: f32 = 0.707_106_77;
pub(super) const LOG_Q1: f32 = -2.121_944_4e-4;
pub(super) const LOG_Q2: f32 = 0.693_359_4;

/// Apply the color-preserving arcsinh curve in place to one band of three RGB-f32 **planes**.
///
/// The three slices must be the same length. Callers split the planes in lockstep and hand each
/// task one band of every channel, so the backend choice is made per band rather than per pixel.
pub(super) fn asinh_color_preserve(
    red: &mut [f32],
    green: &mut [f32],
    blue: &mut [f32],
    c: AsinhCurve,
) {
    dispatch! {
        x86: avx2_fma => avx2::asinh_color_preserve_avx2(red, green, blue, c.inv_beta, c.inv_norm),
        aarch64 => neon::asinh_color_preserve_neon(red, green, blue, c.inv_beta, c.inv_norm),
        scalar => asinh_color_preserve_scalar(red, green, blue, c),
    }
}

/// Scalar counterpart of the vectorized kernels. Same per-pixel curve as
/// [`color_preserve_pixel`], over the same per-band split, so every backend sees the identical
/// work division.
fn asinh_color_preserve_scalar(
    red: &mut [f32],
    green: &mut [f32],
    blue: &mut [f32],
    c: AsinhCurve,
) {
    for ((r, g), b) in red.iter_mut().zip(green.iter_mut()).zip(blue.iter_mut()) {
        let out = color_preserve_pixel(
            Rgb {
                r: *r,
                g: *g,
                b: *b,
            },
            &c,
        );
        *r = out.r;
        *g = out.g;
        *b = out.b;
    }
}
