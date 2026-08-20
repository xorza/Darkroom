//! SSE2 and SSSE3 row conversion implementations for x86_64.

#![allow(unsafe_op_in_unsafe_fn)]

use std::arch::x86_64::*;

use crate::image::conversion::simd;
use crate::image::conversion::{LUMA_B, LUMA_G, LUMA_R};


#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgba_to_rgb_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let shuffle = _mm_setr_epi8(0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1);

    for i in 0..simd_width {
        let src_offset = i * 64;
        let dst_offset = i * 48;

        let rgba0 = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
        let rgba1 = _mm_loadu_si128(src_ptr.add(src_offset + 16) as *const __m128i);
        let rgba2 = _mm_loadu_si128(src_ptr.add(src_offset + 32) as *const __m128i);
        let rgba3 = _mm_loadu_si128(src_ptr.add(src_offset + 48) as *const __m128i);

        let rgb0 = _mm_shuffle_epi8(rgba0, shuffle);
        let rgb1 = _mm_shuffle_epi8(rgba1, shuffle);
        let rgb2 = _mm_shuffle_epi8(rgba2, shuffle);
        let rgb3 = _mm_shuffle_epi8(rgba3, shuffle);

        let out0 = _mm_or_si128(rgb0, _mm_slli_si128(rgb1, 12));
        let out1 = _mm_or_si128(_mm_srli_si128(rgb1, 4), _mm_slli_si128(rgb2, 8));
        let out2 = _mm_or_si128(_mm_srli_si128(rgb2, 8), _mm_slli_si128(rgb3, 4));

        _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, out0);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, out1);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 32) as *mut __m128i, out2);
    }

    // Scalar remainder
    let src_rem = &src[simd_width * 64..];
    let dst_rem = &mut dst[simd_width * 48..];
    for i in 0..remainder {
        dst_rem[i * 3] = src_rem[i * 4];
        dst_rem[i * 3 + 1] = src_rem[i * 4 + 1];
        dst_rem[i * 3 + 2] = src_rem[i * 4 + 2];
    }
}

#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgb_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let alpha_mask = _mm_setr_epi8(0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1);
    let shuf = _mm_setr_epi8(0, 1, 2, -1, 3, 4, 5, -1, 6, 7, 8, -1, 9, 10, 11, -1);

    for i in 0..simd_width {
        let src_offset = i * 48;
        let dst_offset = i * 64;

        let in0 = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
        let in1 = _mm_loadu_si128(src_ptr.add(src_offset + 16) as *const __m128i);
        let in2 = _mm_loadu_si128(src_ptr.add(src_offset + 32) as *const __m128i);

        let rgba0 = _mm_or_si128(_mm_shuffle_epi8(in0, shuf), alpha_mask);
        let combined1 = _mm_or_si128(_mm_srli_si128(in0, 12), _mm_slli_si128(in1, 4));
        let rgba1 = _mm_or_si128(_mm_shuffle_epi8(combined1, shuf), alpha_mask);
        let combined2 = _mm_or_si128(_mm_srli_si128(in1, 8), _mm_slli_si128(in2, 8));
        let rgba2 = _mm_or_si128(_mm_shuffle_epi8(combined2, shuf), alpha_mask);
        let combined3 = _mm_srli_si128(in2, 4);
        let rgba3 = _mm_or_si128(_mm_shuffle_epi8(combined3, shuf), alpha_mask);

        _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, rgba0);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, rgba1);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 32) as *mut __m128i, rgba2);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 48) as *mut __m128i, rgba3);
    }

    let src_rem = &src[simd_width * 48..];
    let dst_rem = &mut dst[simd_width * 64..];
    for i in 0..remainder {
        dst_rem[i * 4] = src_rem[i * 3];
        dst_rem[i * 4 + 1] = src_rem[i * 3 + 1];
        dst_rem[i * 4 + 2] = src_rem[i * 3 + 2];
        dst_rem[i * 4 + 3] = 255;
    }
}

/// The Rec. 709 luma weights arranged for `madd`, together with the shuffle
/// masks that gather one pixel layout's channels into the lanes they expect.
///
/// `madd` multiplies *signed* 16-bit lanes: red and blue ride one lane pair, and
/// green — which overflows `i16` on its own — rides a second pair as two halves
/// that sum back to it. It accumulates into `i32`, so the weighted sum is the
/// scalar reference's exactly, not the 8-bit approximation a 16-bit accumulator
/// would force.
#[derive(Debug, Clone, Copy)]
struct LumaWeights {
    weight_rb: __m128i,
    weight_gg: __m128i,
    gather_rb: __m128i,
    gather_gg: __m128i,
}

// Guards the split above: `madd` would misread a weight that does not fit.
const _: () = assert!(LUMA_R <= i16::MAX as u32);
const _: () = assert!(LUMA_B <= i16::MAX as u32);
const _: () = assert!(LUMA_G - LUMA_G / 2 <= i16::MAX as u32);

impl LumaWeights {
    /// For four `RGBA_U8` pixels held from byte zero.
    #[target_feature(enable = "ssse3")]
    fn rgba() -> Self {
        Self::new(
            _mm_setr_epi8(0, -1, 2, -1, 4, -1, 6, -1, 8, -1, 10, -1, 12, -1, 14, -1),
            _mm_setr_epi8(1, -1, 1, -1, 5, -1, 5, -1, 9, -1, 9, -1, 13, -1, 13, -1),
        )
    }

    /// For four `RGB_U8` pixels held from byte zero.
    #[target_feature(enable = "ssse3")]
    fn rgb() -> Self {
        Self::new(
            _mm_setr_epi8(0, -1, 2, -1, 3, -1, 5, -1, 6, -1, 8, -1, 9, -1, 11, -1),
            _mm_setr_epi8(1, -1, 1, -1, 4, -1, 4, -1, 7, -1, 7, -1, 10, -1, 10, -1),
        )
    }

    #[target_feature(enable = "ssse3")]
    fn new(gather_rb: __m128i, gather_gg: __m128i) -> Self {
        let (r, b) = (LUMA_R as i16, LUMA_B as i16);
        let (g_lo, g_hi) = ((LUMA_G / 2) as i16, (LUMA_G - LUMA_G / 2) as i16);
        Self {
            weight_rb: _mm_setr_epi16(r, b, r, b, r, b, r, b),
            weight_gg: _mm_setr_epi16(g_lo, g_hi, g_lo, g_hi, g_lo, g_hi, g_lo, g_hi),
            gather_rb,
            gather_gg,
        }
    }

    /// The luminance of the four pixels `quad` holds from byte zero, one per
    /// `i32` lane.
    #[inline]
    #[target_feature(enable = "ssse3")]
    fn apply(self, quad: __m128i) -> __m128i {
        let sum = _mm_add_epi32(
            _mm_madd_epi16(_mm_shuffle_epi8(quad, self.gather_rb), self.weight_rb),
            _mm_madd_epi16(_mm_shuffle_epi8(quad, self.gather_gg), self.weight_gg),
        );
        _mm_srli_epi32::<16>(sum)
    }
}

/// Sixteen luminance values, four `i32` lanes at a time, packed into 16 bytes.
/// Every value is already in `0..=255`, so neither saturating pack can bite.
#[inline]
#[target_feature(enable = "ssse3")]
fn pack_quads(lum: [__m128i; 4]) -> __m128i {
    _mm_packus_epi16(
        _mm_packs_epi32(lum[0], lum[1]),
        _mm_packs_epi32(lum[2], lum[3]),
    )
}

/// One 16-byte load.
#[inline]
#[target_feature(enable = "ssse3")]
unsafe fn load(bytes: &[u8; 16]) -> __m128i {
    // SAFETY: the argument is exactly the 16 bytes the load reads.
    unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) }
}

/// Sixteen `RGBA_U8` pixels per iteration: four 16-byte loads, each already a
/// four-pixel quad, and one 16-byte store.
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgba_to_l_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    let weights = LumaWeights::rgba();

    // `src` runs to the end of the image; only this row's pixels are ours.
    let src = &src[..width * 4];
    let (groups, src_tail) = src.as_chunks::<64>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups) {
        let (parts, _) = group.as_chunks::<16>();
        // SAFETY: a group is exactly 64 bytes, so it splits into four whole
        // loads and the store fills its whole 16-byte chunk.
        unsafe {
            let lum = [
                weights.apply(load(&parts[0])),
                weights.apply(load(&parts[1])),
                weights.apply(load(&parts[2])),
                weights.apply(load(&parts[3])),
            ];
            _mm_storeu_si128(out.as_mut_ptr().cast(), pack_quads(lum));
        }
    }

    simd::luma_tail::<4>(src_tail, dst_tail);
}

/// Sixteen `RGB_U8` pixels per iteration: three 16-byte loads spanning the
/// group's 48 bytes exactly, realigned into four-pixel quads, one 16-byte store.
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgb_to_l_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    let weights = LumaWeights::rgb();

    // `src` runs to the end of the image; only this row's pixels are ours.
    let src = &src[..width * 3];
    let (groups, src_tail) = src.as_chunks::<48>();
    let (out_groups, dst_tail) = dst.as_chunks_mut::<16>();

    for (group, out) in groups.iter().zip(out_groups) {
        let (parts, _) = group.as_chunks::<16>();
        // SAFETY: a group is exactly 48 bytes — three whole loads — and the
        // store fills its whole 16-byte chunk.
        unsafe {
            let (in0, in1, in2) = (load(&parts[0]), load(&parts[1]), load(&parts[2]));
            // A quad is 12 bytes, so only the first starts on a load boundary;
            // `alignr` slides the next three down to byte zero.
            let lum = [
                weights.apply(in0),
                weights.apply(_mm_alignr_epi8::<12>(in1, in0)),
                weights.apply(_mm_alignr_epi8::<8>(in2, in1)),
                weights.apply(_mm_srli_si128::<4>(in2)),
            ];
            _mm_storeu_si128(out.as_mut_ptr().cast(), pack_quads(lum));
        }
    }

    simd::luma_tail::<3>(src_tail, dst_tail);
}

#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_l_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let shuf0 = _mm_setr_epi8(0, 0, 0, -1, 1, 1, 1, -1, 2, 2, 2, -1, 3, 3, 3, -1);
    let shuf1 = _mm_setr_epi8(4, 4, 4, -1, 5, 5, 5, -1, 6, 6, 6, -1, 7, 7, 7, -1);
    let shuf2 = _mm_setr_epi8(8, 8, 8, -1, 9, 9, 9, -1, 10, 10, 10, -1, 11, 11, 11, -1);
    let shuf3 = _mm_setr_epi8(
        12, 12, 12, -1, 13, 13, 13, -1, 14, 14, 14, -1, 15, 15, 15, -1,
    );
    let alpha_mask = _mm_setr_epi8(0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 64;

        let l = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);

        let rgba0 = _mm_or_si128(_mm_shuffle_epi8(l, shuf0), alpha_mask);
        let rgba1 = _mm_or_si128(_mm_shuffle_epi8(l, shuf1), alpha_mask);
        let rgba2 = _mm_or_si128(_mm_shuffle_epi8(l, shuf2), alpha_mask);
        let rgba3 = _mm_or_si128(_mm_shuffle_epi8(l, shuf3), alpha_mask);

        _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, rgba0);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, rgba1);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 32) as *mut __m128i, rgba2);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 48) as *mut __m128i, rgba3);
    }

    let src_rem = &src[simd_width * 16..];
    let dst_rem = &mut dst[simd_width * 64..];
    for i in 0..remainder {
        let l = src_rem[i];
        dst_rem[i * 4] = l;
        dst_rem[i * 4 + 1] = l;
        dst_rem[i * 4 + 2] = l;
        dst_rem[i * 4 + 3] = 255;
    }
}

#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_l_to_rgb_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 48;

        let l = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);

        let shuf_out0 = _mm_setr_epi8(0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5);
        let shuf_out1 = _mm_setr_epi8(5, 5, 6, 6, 6, 7, 7, 7, 8, 8, 8, 9, 9, 9, 10, 10);
        let shuf_out2 = _mm_setr_epi8(
            10, 11, 11, 11, 12, 12, 12, 13, 13, 13, 14, 14, 14, 15, 15, 15,
        );

        let out0 = _mm_shuffle_epi8(l, shuf_out0);
        let out1 = _mm_shuffle_epi8(l, shuf_out1);
        let out2 = _mm_shuffle_epi8(l, shuf_out2);

        _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, out0);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, out1);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 32) as *mut __m128i, out2);
    }

    let src_rem = &src[simd_width * 16..];
    let dst_rem = &mut dst[simd_width * 48..];
    for i in 0..remainder {
        let l = src_rem[i];
        dst_rem[i * 3] = l;
        dst_rem[i * 3 + 1] = l;
        dst_rem[i * 3 + 2] = l;
    }
}


#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_f32_to_u8_row_sse2(src: &[f32], dst: &mut [u8]) {

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    let scale = _mm_set1_ps(255.0);
    let zero_f = _mm_setzero_ps();
    let max_f = _mm_set1_ps(255.0);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let f0 = _mm_loadu_ps(src.as_ptr().add(src_offset));
        let f1 = _mm_loadu_ps(src.as_ptr().add(src_offset + 4));
        let f2 = _mm_loadu_ps(src.as_ptr().add(src_offset + 8));
        let f3 = _mm_loadu_ps(src.as_ptr().add(src_offset + 12));

        let scaled0 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f0, scale), zero_f), max_f);
        let scaled1 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f1, scale), zero_f), max_f);
        let scaled2 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f2, scale), zero_f), max_f);
        let scaled3 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f3, scale), zero_f), max_f);

        let i0 = _mm_cvtps_epi32(scaled0);
        let i1 = _mm_cvtps_epi32(scaled1);
        let i2 = _mm_cvtps_epi32(scaled2);
        let i3 = _mm_cvtps_epi32(scaled3);

        let words_lo = _mm_packs_epi32(i0, i1);
        let words_hi = _mm_packs_epi32(i2, i3);
        let bytes = _mm_packus_epi16(words_lo, words_hi);

        _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, bytes);
    }

    for i in 0..remainder {
        let val = (src[simd_width * 16 + i] * 255.0).round_ties_even().clamp(0.0, 255.0) as u8;
        dst[simd_width * 16 + i] = val;
    }
}

#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u8_to_f32_row_sse2(src: &[u8], dst: &mut [f32]) {

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    // Divide, never a reciprocal multiply — see the module doc on precision.
    let divisor = _mm_set1_ps(255.0);
    let zero = _mm_setzero_si128();

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let bytes = _mm_loadu_si128(src.as_ptr().add(src_offset) as *const __m128i);

        // Unpack bytes to 16-bit words
        let words_lo = _mm_unpacklo_epi8(bytes, zero);
        let words_hi = _mm_unpackhi_epi8(bytes, zero);

        // Unpack 16-bit words to 32-bit dwords
        let dwords_0 = _mm_unpacklo_epi16(words_lo, zero);
        let dwords_1 = _mm_unpackhi_epi16(words_lo, zero);
        let dwords_2 = _mm_unpacklo_epi16(words_hi, zero);
        let dwords_3 = _mm_unpackhi_epi16(words_hi, zero);

        // Convert to float and scale
        let floats_0 = _mm_div_ps(_mm_cvtepi32_ps(dwords_0), divisor);
        let floats_1 = _mm_div_ps(_mm_cvtepi32_ps(dwords_1), divisor);
        let floats_2 = _mm_div_ps(_mm_cvtepi32_ps(dwords_2), divisor);
        let floats_3 = _mm_div_ps(_mm_cvtepi32_ps(dwords_3), divisor);

        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset), floats_0);
        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 4), floats_1);
        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 8), floats_2);
        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 12), floats_3);
    }

    for i in 0..remainder {
        dst[simd_width * 16 + i] = src[simd_width * 16 + i] as f32 / 255.0;
    }
}

#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u8_to_u16_row_sse2(src: &[u8], dst: &mut [u16]) {

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let bytes = _mm_loadu_si128(src.as_ptr().add(src_offset) as *const __m128i);
        let zero = _mm_setzero_si128();
        let words_lo = _mm_unpacklo_epi8(bytes, zero);
        let words_hi = _mm_unpackhi_epi8(bytes, zero);

        let scaled_lo = _mm_or_si128(words_lo, _mm_slli_epi16(words_lo, 8));
        let scaled_hi = _mm_or_si128(words_hi, _mm_slli_epi16(words_hi, 8));

        _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, scaled_lo);
        _mm_storeu_si128(
            dst.as_mut_ptr().add(dst_offset + 8) as *mut __m128i,
            scaled_hi,
        );
    }

    for i in 0..remainder {
        dst[simd_width * 16 + i] = (src[simd_width * 16 + i] as u16) * 257;
    }
}

#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u16_to_u8_row_sse2(src: &[u16], dst: &mut [u8]) {

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let words_lo = _mm_loadu_si128(src.as_ptr().add(src_offset) as *const __m128i);
        let words_hi = _mm_loadu_si128(src.as_ptr().add(src_offset + 8) as *const __m128i);

        let shifted_lo = _mm_srli_epi16(words_lo, 8);
        let shifted_hi = _mm_srli_epi16(words_hi, 8);

        let bytes = _mm_packus_epi16(shifted_lo, shifted_hi);
        _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, bytes);
    }

    for i in 0..remainder {
        dst[simd_width * 16 + i] = (src[simd_width * 16 + i] / 257) as u8;
    }
}

#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u16_to_f32_row_sse2(src: &[u16], dst: &mut [f32]) {

    let len = src.len();
    let simd_width = len / 8;
    let remainder = len % 8;

    // Divide, never a reciprocal multiply — see the module doc on precision.
    let divisor = _mm_set1_ps(65535.0);
    let zero = _mm_setzero_si128();

    for i in 0..simd_width {
        let src_offset = i * 8;
        let dst_offset = i * 8;

        let words = _mm_loadu_si128(src.as_ptr().add(src_offset) as *const __m128i);
        let dwords_lo = _mm_unpacklo_epi16(words, zero);
        let dwords_hi = _mm_unpackhi_epi16(words, zero);

        let floats_lo = _mm_div_ps(_mm_cvtepi32_ps(dwords_lo), divisor);
        let floats_hi = _mm_div_ps(_mm_cvtepi32_ps(dwords_hi), divisor);

        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset), floats_lo);
        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 4), floats_hi);
    }

    for i in 0..remainder {
        dst[simd_width * 8 + i] = src[simd_width * 8 + i] as f32 / 65535.0;
    }
}

#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn convert_f32_to_u16_row_sse41(src: &[f32], dst: &mut [u16]) {

    let len = src.len();
    let simd_width = len / 8;
    let remainder = len % 8;

    let scale = _mm_set1_ps(65535.0);
    let zero_f = _mm_setzero_ps();
    let max_f = _mm_set1_ps(65535.0);

    for i in 0..simd_width {
        let src_offset = i * 8;
        let dst_offset = i * 8;

        let f0 = _mm_loadu_ps(src.as_ptr().add(src_offset));
        let f1 = _mm_loadu_ps(src.as_ptr().add(src_offset + 4));

        let scaled0 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f0, scale), zero_f), max_f);
        let scaled1 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f1, scale), zero_f), max_f);

        let i0 = _mm_cvtps_epi32(scaled0);
        let i1 = _mm_cvtps_epi32(scaled1);

        // Pack with unsigned saturation (SSE4.1) — values are in [0, 65535]
        let words = _mm_packus_epi32(i0, i1);
        _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, words);
    }

    for i in 0..remainder {
        dst[simd_width * 8 + i] = (src[simd_width * 8 + i] * 65535.0).round_ties_even().clamp(0.0, 65535.0) as u16;
    }
}
