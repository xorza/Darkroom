//! SSE4.1 blend kernels for `RGBA_U8` and `RGBA_F32`.
//!
//! One pixel's four channels fill one register, so the blend is a single
//! expression both storage types share — only the load and the store differ.
//!
//! Every step matches the scalar reference operation for operation, so the
//! vector results are bit-identical to it rather than within a rounding
//! tolerance: the byte path *divides* by 255 where multiplying by the reciprocal
//! would be faster, and the alpha mix is a separate multiply and add. Each
//! kernel finishes its sub-vector tail with the reference itself.

use std::arch::x86_64::*;

use crate::ops::blend::cpu;
use crate::ops::blend::{Blend, BlendMode};

/// The blend's constants, splatted once per row. `max` is the storage type's
/// full-scale value: 255 for bytes, one for floats.
#[derive(Debug, Clone, Copy)]
struct Splat {
    alpha: __m128,
    one_minus_alpha: __m128,
    zero: __m128,
    one: __m128,
    half: __m128,
    two: __m128,
    max: __m128,
}

impl Splat {
    #[target_feature(enable = "sse4.1")]
    fn new(alpha: f32, max: f32) -> Self {
        Self {
            alpha: _mm_set1_ps(alpha),
            one_minus_alpha: _mm_set1_ps(1.0 - alpha),
            zero: _mm_setzero_ps(),
            one: _mm_set1_ps(1.0),
            half: _mm_set1_ps(0.5),
            two: _mm_set1_ps(2.0),
            max: _mm_set1_ps(max),
        }
    }

    /// One pixel's four normalized channels, blended and mixed — the vector form
    /// of [`BlendMode::blend`].
    ///
    /// Lane 3 is alpha, which carries no mode of its own. `Normal`'s blended
    /// value is `src`, so restoring src in that lane is the whole difference.
    #[inline]
    #[target_feature(enable = "sse4.1")]
    fn blend(self, mode: BlendMode, src: __m128, dst: __m128) -> __m128 {
        let blended = match mode {
            BlendMode::Normal => src,
            BlendMode::Add => _mm_min_ps(_mm_add_ps(src, dst), self.one),
            BlendMode::Subtract => _mm_max_ps(_mm_sub_ps(dst, src), self.zero),
            BlendMode::Multiply => _mm_mul_ps(src, dst),
            BlendMode::Screen => _mm_sub_ps(
                self.one,
                _mm_mul_ps(_mm_sub_ps(self.one, src), _mm_sub_ps(self.one, dst)),
            ),
            BlendMode::Overlay => {
                let dark = _mm_mul_ps(self.two, _mm_mul_ps(src, dst));
                let light = _mm_sub_ps(
                    self.one,
                    _mm_mul_ps(
                        self.two,
                        _mm_mul_ps(_mm_sub_ps(self.one, src), _mm_sub_ps(self.one, dst)),
                    ),
                );
                _mm_blendv_ps(light, dark, _mm_cmplt_ps(dst, self.half))
            }
        };
        let blended = _mm_blend_ps::<0b1000>(blended, src);
        _mm_add_ps(
            _mm_mul_ps(blended, self.alpha),
            _mm_mul_ps(dst, self.one_minus_alpha),
        )
    }

    /// Scales a blended value back into the storage type's range and clamps it
    /// there, in the reference's order: multiply, then clamp the product. For
    /// float storage `max` is one, so the multiply is exact and the clamp is the
    /// whole step.
    #[inline]
    #[target_feature(enable = "sse4.1")]
    fn scale_clamp(self, blended: __m128) -> __m128 {
        _mm_min_ps(
            _mm_max_ps(_mm_mul_ps(blended, self.max), self.zero),
            self.max,
        )
    }

    /// The four channels of the pixel starting at byte `BYTE` of a four-pixel
    /// register, widened and divided into `[0, 1]`.
    ///
    /// The divide is what the reference does. `value * (1.0 / 255.0)` would be
    /// several times cheaper but disagrees with it for 126 of the 256 byte
    /// values, which is enough to shift an output byte by one once the result is
    /// scaled back up and truncated.
    #[inline]
    #[target_feature(enable = "sse4.1")]
    fn normalize<const BYTE: i32>(self, pixels: __m128i) -> __m128 {
        // `cvtepu8_epi32` widens the low four bytes, so bring the wanted pixel
        // down to them first.
        let pixel = _mm_srli_si128::<BYTE>(pixels);
        _mm_div_ps(_mm_cvtepi32_ps(_mm_cvtepu8_epi32(pixel)), self.max)
    }

    /// One `RGBA_U8` pixel of the four in `src`/`dst`, blended and returned as
    /// four `i32` lanes ready to pack. `cvttps` truncates toward zero, which is
    /// what the reference's `as u8` does.
    #[inline]
    #[target_feature(enable = "sse4.1")]
    fn blend_u8<const BYTE: i32>(self, mode: BlendMode, src: __m128i, dst: __m128i) -> __m128i {
        let src = self.normalize::<BYTE>(src);
        let dst = self.normalize::<BYTE>(dst);
        _mm_cvttps_epi32(self.scale_clamp(self.blend(mode, src, dst)))
    }
}

/// Four `RGBA_U8` pixels per iteration: one 16-byte load from each row, four
/// pixel registers, one 16-byte store.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn rgba_u8_row(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    params: Blend,
) {
    let splat = Splat::new(params.alpha, f32::from(u8::MAX));
    let mode = params.mode;

    let (src_quads, src_tail) = src_row.as_chunks::<16>();
    let (dst_quads, dst_tail) = dst_row.as_chunks::<16>();
    let (out_quads, out_tail) = out_row.as_chunks_mut::<16>();

    for ((src, dst), out) in src_quads.iter().zip(dst_quads).zip(out_quads) {
        // SAFETY: every chunk is exactly 16 bytes — four RGBA pixels, the width
        // of one load and one store.
        unsafe {
            let src = _mm_loadu_si128(src.as_ptr().cast());
            let dst = _mm_loadu_si128(dst.as_ptr().cast());

            let p0 = splat.blend_u8::<0>(mode, src, dst);
            let p1 = splat.blend_u8::<4>(mode, src, dst);
            let p2 = splat.blend_u8::<8>(mode, src, dst);
            let p3 = splat.blend_u8::<12>(mode, src, dst);

            let lo = _mm_packus_epi32(p0, p1);
            let hi = _mm_packus_epi32(p2, p3);
            _mm_storeu_si128(out.as_mut_ptr().cast(), _mm_packus_epi16(lo, hi));
        }
    }

    cpu::rgba_tail(params, src_tail, dst_tail, out_tail);
}

/// One `RGBA_F32` pixel per iteration — four floats already fill a register, so
/// there is nothing to widen and no tail to finish.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn rgba_f32_row(
    src_row: &[u8],
    dst_row: &[u8],
    out_row: &mut [u8],
    params: Blend,
) {
    let splat = Splat::new(params.alpha, 1.0);
    let mode = params.mode;

    let src_row: &[f32] = bytemuck::cast_slice(src_row);
    let dst_row: &[f32] = bytemuck::cast_slice(dst_row);
    let out_row: &mut [f32] = bytemuck::cast_slice_mut(out_row);

    let (src_pixels, rest) = src_row.as_chunks::<4>();
    let (dst_pixels, _) = dst_row.as_chunks::<4>();
    let (out_pixels, _) = out_row.as_chunks_mut::<4>();
    debug_assert!(rest.is_empty(), "an RGBA row is a whole number of pixels");

    for ((src, dst), out) in src_pixels.iter().zip(dst_pixels).zip(out_pixels) {
        // SAFETY: every chunk is exactly four `f32` — one RGBA pixel, the width
        // of one load and one store.
        unsafe {
            let src = _mm_loadu_ps(src.as_ptr());
            let dst = _mm_loadu_ps(dst.as_ptr());
            _mm_storeu_ps(
                out.as_mut_ptr(),
                splat.scale_clamp(splat.blend(mode, src, dst)),
            );
        }
    }
}
