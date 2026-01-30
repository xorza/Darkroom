// SSE2 and SSSE3 implementations for row conversion
//
// This module contains SSE2 and SSSE3 SIMD implementations for x86_64.

#![allow(unsafe_op_in_unsafe_fn)]

use super::LUMA_B;
use super::LUMA_G;
use super::LUMA_R;

// =============================================================================
// SSSE3 implementations
// =============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgba_to_rgb_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgb_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgba_to_l_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 8;
    let remainder = width % 8;

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 8;

        let rgba0 = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
        let rgba1 = _mm_loadu_si128(src_ptr.add(src_offset + 16) as *const __m128i);

        let shuf_r = _mm_setr_epi8(0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_g = _mm_setr_epi8(1, 5, 9, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_b = _mm_setr_epi8(2, 6, 10, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

        let r0 = _mm_shuffle_epi8(rgba0, shuf_r);
        let g0 = _mm_shuffle_epi8(rgba0, shuf_g);
        let b0 = _mm_shuffle_epi8(rgba0, shuf_b);
        let r1 = _mm_shuffle_epi8(rgba1, shuf_r);
        let g1 = _mm_shuffle_epi8(rgba1, shuf_g);
        let b1 = _mm_shuffle_epi8(rgba1, shuf_b);

        let r = _mm_or_si128(r0, _mm_slli_si128(r1, 4));
        let g = _mm_or_si128(g0, _mm_slli_si128(g1, 4));
        let b = _mm_or_si128(b0, _mm_slli_si128(b1, 4));

        let zero = _mm_setzero_si128();
        let r16 = _mm_unpacklo_epi8(r, zero);
        let g16 = _mm_unpacklo_epi8(g, zero);
        let b16 = _mm_unpacklo_epi8(b, zero);

        let r_w = _mm_set1_epi16(54);
        let g_w = _mm_set1_epi16(183);
        let b_w = _mm_set1_epi16(19);

        let sum = _mm_add_epi16(
            _mm_add_epi16(_mm_mullo_epi16(r16, r_w), _mm_mullo_epi16(g16, g_w)),
            _mm_mullo_epi16(b16, b_w),
        );
        let lum16 = _mm_srli_epi16(sum, 8);
        let lum8 = _mm_packus_epi16(lum16, zero);

        _mm_storel_epi64(dst_ptr.add(dst_offset) as *mut __m128i, lum8);
    }

    let src_rem = &src[simd_width * 32..];
    let dst_rem = &mut dst[simd_width * 8..];
    for i in 0..remainder {
        let r = src_rem[i * 4] as u32;
        let g = src_rem[i * 4 + 1] as u32;
        let b = src_rem[i * 4 + 2] as u32;
        dst_rem[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgb_to_l_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    for i in 0..simd_width {
        let src_offset = i * 48;
        let dst_offset = i * 16;

        let in0 = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
        let in1 = _mm_loadu_si128(src_ptr.add(src_offset + 16) as *const __m128i);
        let in2 = _mm_loadu_si128(src_ptr.add(src_offset + 32) as *const __m128i);

        // Extract R, G, B for first 8 pixels
        let shuf_r0 = _mm_setr_epi8(0, 3, 6, 9, 12, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_g0 = _mm_setr_epi8(1, 4, 7, 10, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_b0 = _mm_setr_epi8(2, 5, 8, 11, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

        let r0_part = _mm_shuffle_epi8(in0, shuf_r0);
        let g0_part = _mm_shuffle_epi8(in0, shuf_g0);
        let b0_part = _mm_shuffle_epi8(in0, shuf_b0);

        let shuf_r1 = _mm_setr_epi8(2, 5, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_g1 = _mm_setr_epi8(0, 3, 6, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_b1 = _mm_setr_epi8(1, 4, 7, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

        let r1_part = _mm_shuffle_epi8(in1, shuf_r1);
        let g1_part = _mm_shuffle_epi8(in1, shuf_g1);
        let b1_part = _mm_shuffle_epi8(in1, shuf_b1);

        let r_lo = _mm_or_si128(r0_part, _mm_slli_si128(r1_part, 6));
        let g_lo = _mm_or_si128(g0_part, _mm_slli_si128(g1_part, 5));
        let b_lo = _mm_or_si128(b0_part, _mm_slli_si128(b1_part, 5));

        // Second 8 pixels
        let shuf_r2 = _mm_setr_epi8(
            8, 11, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );
        let shuf_g2 = _mm_setr_epi8(
            9, 12, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );
        let shuf_b2 = _mm_setr_epi8(
            10, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );

        let r2_part = _mm_shuffle_epi8(in1, shuf_r2);
        let g2_part = _mm_shuffle_epi8(in1, shuf_g2);
        let b2_part = _mm_shuffle_epi8(in1, shuf_b2);

        let shuf_r3 = _mm_setr_epi8(1, 4, 7, 10, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_g3 = _mm_setr_epi8(2, 5, 8, 11, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_b3 = _mm_setr_epi8(0, 3, 6, 9, 12, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

        let r3_part = _mm_shuffle_epi8(in2, shuf_r3);
        let g3_part = _mm_shuffle_epi8(in2, shuf_g3);
        let b3_part = _mm_shuffle_epi8(in2, shuf_b3);

        let r_hi = _mm_or_si128(r2_part, _mm_slli_si128(r3_part, 3));
        let g_hi = _mm_or_si128(g2_part, _mm_slli_si128(g3_part, 3));
        let b_hi = _mm_or_si128(b2_part, _mm_slli_si128(b3_part, 2));

        let zero = _mm_setzero_si128();
        let r_w = _mm_set1_epi16(54);
        let g_w = _mm_set1_epi16(183);
        let b_w = _mm_set1_epi16(19);

        let r16_lo = _mm_unpacklo_epi8(r_lo, zero);
        let g16_lo = _mm_unpacklo_epi8(g_lo, zero);
        let b16_lo = _mm_unpacklo_epi8(b_lo, zero);

        let sum_lo = _mm_add_epi16(
            _mm_add_epi16(_mm_mullo_epi16(r16_lo, r_w), _mm_mullo_epi16(g16_lo, g_w)),
            _mm_mullo_epi16(b16_lo, b_w),
        );
        let lum16_lo = _mm_srli_epi16(sum_lo, 8);

        let r16_hi = _mm_unpacklo_epi8(r_hi, zero);
        let g16_hi = _mm_unpacklo_epi8(g_hi, zero);
        let b16_hi = _mm_unpacklo_epi8(b_hi, zero);

        let sum_hi = _mm_add_epi16(
            _mm_add_epi16(_mm_mullo_epi16(r16_hi, r_w), _mm_mullo_epi16(g16_hi, g_w)),
            _mm_mullo_epi16(b16_hi, b_w),
        );
        let lum16_hi = _mm_srli_epi16(sum_hi, 8);

        let lum8 = _mm_packus_epi16(lum16_lo, lum16_hi);
        _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, lum8);
    }

    let src_rem = &src[simd_width * 48..];
    let dst_rem = &mut dst[simd_width * 16..];
    for i in 0..remainder {
        let r = src_rem[i * 3] as u32;
        let g = src_rem[i * 3 + 1] as u32;
        let b = src_rem[i * 3 + 2] as u32;
        dst_rem[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_l_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_l_to_rgb_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_la_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 8;
    let remainder = width % 8;

    // LA -> RGBA: L L L A
    let shuf = _mm_setr_epi8(0, 0, 0, 1, 2, 2, 2, 3, 4, 4, 4, 5, 6, 6, 6, 7);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 32;

        let la = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
        let rgba0 = _mm_shuffle_epi8(la, shuf);
        let shuf_hi = _mm_setr_epi8(8, 8, 8, 9, 10, 10, 10, 11, 12, 12, 12, 13, 14, 14, 14, 15);
        let rgba1 = _mm_shuffle_epi8(la, shuf_hi);

        _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, rgba0);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, rgba1);
    }

    let src_rem = &src[simd_width * 16..];
    let dst_rem = &mut dst[simd_width * 32..];
    for i in 0..remainder {
        let l = src_rem[i * 2];
        let a = src_rem[i * 2 + 1];
        dst_rem[i * 4] = l;
        dst_rem[i * 4 + 1] = l;
        dst_rem[i * 4 + 2] = l;
        dst_rem[i * 4 + 3] = a;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
pub(super) unsafe fn convert_rgba_to_la_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 4;
    let remainder = width % 4;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 8;

        let rgba = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);

        // Extract R, G, B, A
        let shuf_r = _mm_setr_epi8(0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_g = _mm_setr_epi8(1, 5, 9, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_b = _mm_setr_epi8(2, 6, 10, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
        let shuf_a = _mm_setr_epi8(3, 7, 11, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

        let r = _mm_shuffle_epi8(rgba, shuf_r);
        let g = _mm_shuffle_epi8(rgba, shuf_g);
        let b = _mm_shuffle_epi8(rgba, shuf_b);
        let a = _mm_shuffle_epi8(rgba, shuf_a);

        // Compute luminance
        let zero = _mm_setzero_si128();
        let r16 = _mm_unpacklo_epi8(r, zero);
        let g16 = _mm_unpacklo_epi8(g, zero);
        let b16 = _mm_unpacklo_epi8(b, zero);

        let r_w = _mm_set1_epi16(54);
        let g_w = _mm_set1_epi16(183);
        let b_w = _mm_set1_epi16(19);

        let sum = _mm_add_epi16(
            _mm_add_epi16(_mm_mullo_epi16(r16, r_w), _mm_mullo_epi16(g16, g_w)),
            _mm_mullo_epi16(b16, b_w),
        );
        let lum16 = _mm_srli_epi16(sum, 8);
        let lum8 = _mm_packus_epi16(lum16, zero);

        // Interleave L and A
        let la = _mm_unpacklo_epi8(lum8, a);
        _mm_storel_epi64(dst_ptr.add(dst_offset) as *mut __m128i, la);
    }

    let src_rem = &src[simd_width * 16..];
    let dst_rem = &mut dst[simd_width * 8..];
    for i in 0..remainder {
        let r = src_rem[i * 4] as u32;
        let g = src_rem[i * 4 + 1] as u32;
        let b = src_rem[i * 4 + 2] as u32;
        let a = src_rem[i * 4 + 3];
        dst_rem[i * 2] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
        dst_rem[i * 2 + 1] = a;
    }
}

// =============================================================================
// SSE2 implementations
// =============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_f32_to_u8_row_sse2(src: &[f32], dst: &mut [u8]) {
    use std::arch::x86_64::*;

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
        let val = (src[simd_width * 16 + i] * 255.0).clamp(0.0, 255.0) as u8;
        dst[simd_width * 16 + i] = val;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u8_to_f32_row_sse2(src: &[u8], dst: &mut [f32]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    let scale = _mm_set1_ps(1.0 / 255.0);
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
        let floats_0 = _mm_mul_ps(_mm_cvtepi32_ps(dwords_0), scale);
        let floats_1 = _mm_mul_ps(_mm_cvtepi32_ps(dwords_1), scale);
        let floats_2 = _mm_mul_ps(_mm_cvtepi32_ps(dwords_2), scale);
        let floats_3 = _mm_mul_ps(_mm_cvtepi32_ps(dwords_3), scale);

        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset), floats_0);
        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 4), floats_1);
        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 8), floats_2);
        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 12), floats_3);
    }

    for i in 0..remainder {
        dst[simd_width * 16 + i] = src[simd_width * 16 + i] as f32 / 255.0;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u8_to_u16_row_sse2(src: &[u8], dst: &mut [u16]) {
    use std::arch::x86_64::*;

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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u16_to_u8_row_sse2(src: &[u16], dst: &mut [u8]) {
    use std::arch::x86_64::*;

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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_u16_to_f32_row_sse2(src: &[u16], dst: &mut [f32]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 8;
    let remainder = len % 8;

    let scale = _mm_set1_ps(1.0 / 65535.0);
    let zero = _mm_setzero_si128();

    for i in 0..simd_width {
        let src_offset = i * 8;
        let dst_offset = i * 8;

        let words = _mm_loadu_si128(src.as_ptr().add(src_offset) as *const __m128i);
        let dwords_lo = _mm_unpacklo_epi16(words, zero);
        let dwords_hi = _mm_unpackhi_epi16(words, zero);

        let floats_lo = _mm_mul_ps(_mm_cvtepi32_ps(dwords_lo), scale);
        let floats_hi = _mm_mul_ps(_mm_cvtepi32_ps(dwords_hi), scale);

        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset), floats_lo);
        _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 4), floats_hi);
    }

    for i in 0..remainder {
        dst[simd_width * 8 + i] = src[simd_width * 8 + i] as f32 / 65535.0;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub(super) unsafe fn convert_f32_to_u16_row_sse2(src: &[f32], dst: &mut [u16]) {
    use std::arch::x86_64::*;

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

        // Pack with signed saturation (values are clamped to [0, 65535] so this is safe)
        let words = _mm_packs_epi32(i0, i1);
        _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, words);
    }

    for i in 0..remainder {
        dst[simd_width * 8 + i] = (src[simd_width * 8 + i] * 65535.0).clamp(0.0, 65535.0) as u16;
    }
}
