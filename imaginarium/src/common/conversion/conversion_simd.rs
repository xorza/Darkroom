// SIMD-optimized row conversion implementations
//
// This module contains SIMD implementations for single-row conversion.
// All functions process one row at a time - parallelization is handled by the caller.
//
// Paths with SIMD:
// - RGBA_U8 <-> RGB_U8 (1.07-1.12x)
// - RGB_U8/RGBA_U8 -> L_U8 (1.65-1.90x)
// - L_U8 -> RGBA_U8/RGB_U8 (2.10x)
// - LA_U8 <-> RGBA_U8 (1.11-1.30x)
// - U8 <-> U16 (1.02-1.03x, kept for consistency)
// - L_U16 <-> F32 (1.06-1.14x)
// - F32 -> U8 (1.02-1.03x, kept for consistency)

#![allow(unsafe_op_in_unsafe_fn)]

use crate::common::color_format::ColorFormat;

// Rec. 709 luminance weights scaled to fixed-point (shift by 16)
const LUMA_R: u32 = 13933;
const LUMA_G: u32 = 46871;
const LUMA_B: u32 = 4732;

/// Row conversion function type
pub(crate) type RowConvertFn = fn(src: &[u8], dst: &mut [u8], width: usize);

/// Get the SIMD row conversion function for a format pair, if available.
/// Returns None if no SIMD path exists for this conversion.
pub(crate) fn get_simd_row_converter(
    from_fmt: ColorFormat,
    to_fmt: ColorFormat,
) -> Option<RowConvertFn> {
    #[cfg(target_arch = "x86_64")]
    {
        if !is_x86_feature_detected!("sse2") {
            return None;
        }

        match (from_fmt, to_fmt) {
            // Channel conversions U8 (require SSSE3)
            (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) if is_x86_feature_detected!("ssse3") => {
                Some(convert_rgba_u8_to_rgb_u8_row)
            }
            (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) if is_x86_feature_detected!("ssse3") => {
                Some(convert_rgb_u8_to_rgba_u8_row)
            }
            // Luminance U8 (require SSSE3)
            (ColorFormat::RGBA_U8, ColorFormat::L_U8) if is_x86_feature_detected!("ssse3") => {
                Some(convert_rgba_u8_to_l_u8_row)
            }
            (ColorFormat::RGB_U8, ColorFormat::L_U8) if is_x86_feature_detected!("ssse3") => {
                Some(convert_rgb_u8_to_l_u8_row)
            }
            // L_U8 expansion (require SSSE3)
            (ColorFormat::L_U8, ColorFormat::RGBA_U8) if is_x86_feature_detected!("ssse3") => {
                Some(convert_l_u8_to_rgba_u8_row)
            }
            (ColorFormat::L_U8, ColorFormat::RGB_U8) if is_x86_feature_detected!("ssse3") => {
                Some(convert_l_u8_to_rgb_u8_row)
            }
            // LA_U8 <-> RGBA_U8 (require SSSE3)
            (ColorFormat::LA_U8, ColorFormat::RGBA_U8) if is_x86_feature_detected!("ssse3") => {
                Some(convert_la_u8_to_rgba_u8_row)
            }
            (ColorFormat::RGBA_U8, ColorFormat::LA_U8) if is_x86_feature_detected!("ssse3") => {
                Some(convert_rgba_u8_to_la_u8_row)
            }
            // F32->U8
            (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => Some(convert_f32_to_u8_row_4ch),
            (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => Some(convert_f32_to_u8_row_3ch),
            (ColorFormat::L_F32, ColorFormat::L_U8) => Some(convert_f32_to_u8_row_1ch),
            (ColorFormat::LA_F32, ColorFormat::LA_U8) => Some(convert_f32_to_u8_row_2ch),
            // U8<->U16
            (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16) => Some(convert_u8_to_u16_row_4ch),
            (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8) => Some(convert_u16_to_u8_row_4ch),
            (ColorFormat::RGB_U8, ColorFormat::RGB_U16) => Some(convert_u8_to_u16_row_3ch),
            (ColorFormat::RGB_U16, ColorFormat::RGB_U8) => Some(convert_u16_to_u8_row_3ch),
            (ColorFormat::L_U8, ColorFormat::L_U16) => Some(convert_u8_to_u16_row_1ch),
            (ColorFormat::L_U16, ColorFormat::L_U8) => Some(convert_u16_to_u8_row_1ch),
            (ColorFormat::LA_U8, ColorFormat::LA_U16) => Some(convert_u8_to_u16_row_2ch),
            (ColorFormat::LA_U16, ColorFormat::LA_U8) => Some(convert_u16_to_u8_row_2ch),
            // U16<->F32 (only L and LA have meaningful speedup)
            (ColorFormat::L_U16, ColorFormat::L_F32) => Some(convert_u16_to_f32_row_1ch),
            (ColorFormat::L_F32, ColorFormat::L_U16) => Some(convert_f32_to_u16_row_1ch),
            (ColorFormat::LA_U16, ColorFormat::LA_F32) => Some(convert_u16_to_f32_row_2ch),
            (ColorFormat::LA_F32, ColorFormat::LA_U16) => Some(convert_f32_to_u16_row_2ch),
            _ => None,
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        match (from_fmt, to_fmt) {
            // Channel conversions U8
            (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => Some(convert_rgba_u8_to_rgb_u8_row),
            (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) => Some(convert_rgb_u8_to_rgba_u8_row),
            // Luminance U8
            (ColorFormat::RGBA_U8, ColorFormat::L_U8) => Some(convert_rgba_u8_to_l_u8_row),
            (ColorFormat::RGB_U8, ColorFormat::L_U8) => Some(convert_rgb_u8_to_l_u8_row),
            // L_U8 expansion
            (ColorFormat::L_U8, ColorFormat::RGBA_U8) => Some(convert_l_u8_to_rgba_u8_row),
            (ColorFormat::L_U8, ColorFormat::RGB_U8) => Some(convert_l_u8_to_rgb_u8_row),
            // LA_U8 <-> RGBA_U8
            (ColorFormat::LA_U8, ColorFormat::RGBA_U8) => Some(convert_la_u8_to_rgba_u8_row),
            (ColorFormat::RGBA_U8, ColorFormat::LA_U8) => Some(convert_rgba_u8_to_la_u8_row),
            // F32->U8
            (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => Some(convert_f32_to_u8_row_4ch),
            (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => Some(convert_f32_to_u8_row_3ch),
            (ColorFormat::L_F32, ColorFormat::L_U8) => Some(convert_f32_to_u8_row_1ch),
            (ColorFormat::LA_F32, ColorFormat::LA_U8) => Some(convert_f32_to_u8_row_2ch),
            // U8<->U16
            (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16) => Some(convert_u8_to_u16_row_4ch),
            (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8) => Some(convert_u16_to_u8_row_4ch),
            (ColorFormat::RGB_U8, ColorFormat::RGB_U16) => Some(convert_u8_to_u16_row_3ch),
            (ColorFormat::RGB_U16, ColorFormat::RGB_U8) => Some(convert_u16_to_u8_row_3ch),
            (ColorFormat::L_U8, ColorFormat::L_U16) => Some(convert_u8_to_u16_row_1ch),
            (ColorFormat::L_U16, ColorFormat::L_U8) => Some(convert_u16_to_u8_row_1ch),
            (ColorFormat::LA_U8, ColorFormat::LA_U16) => Some(convert_u8_to_u16_row_2ch),
            (ColorFormat::LA_U16, ColorFormat::LA_U8) => Some(convert_u16_to_u8_row_2ch),
            // U16<->F32
            (ColorFormat::L_U16, ColorFormat::L_F32) => Some(convert_u16_to_f32_row_1ch),
            (ColorFormat::L_F32, ColorFormat::L_U16) => Some(convert_f32_to_u16_row_1ch),
            (ColorFormat::LA_U16, ColorFormat::LA_F32) => Some(convert_u16_to_f32_row_2ch),
            (ColorFormat::LA_F32, ColorFormat::LA_U16) => Some(convert_f32_to_u16_row_2ch),
            _ => None,
        }
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (from_fmt, to_fmt);
        None
    }
}

// =============================================================================
// Row conversion wrapper functions
// These dispatch to the appropriate SIMD implementation based on CPU features.
// =============================================================================

fn convert_rgba_u8_to_rgb_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_rgba_to_rgb_row_avx2(src, dst, width);
        } else {
            convert_rgba_to_rgb_row_ssse3(src, dst, width);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_rgba_to_rgb_row_neon(src, dst, width);
    }
}

fn convert_rgb_u8_to_rgba_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_rgb_to_rgba_row_avx2(src, dst, width);
        } else {
            convert_rgb_to_rgba_row_ssse3(src, dst, width);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_rgb_to_rgba_row_neon(src, dst, width);
    }
}

fn convert_rgba_u8_to_l_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_rgba_to_l_row_avx2(src, dst, width);
        } else {
            convert_rgba_to_l_row_ssse3(src, dst, width);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_rgba_to_l_row_neon(src, dst, width);
    }
}

fn convert_rgb_u8_to_l_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_rgb_to_l_row_avx2(src, dst, width);
        } else {
            convert_rgb_to_l_row_ssse3(src, dst, width);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_rgb_to_l_row_neon(src, dst, width);
    }
}

fn convert_l_u8_to_rgba_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_l_to_rgba_row_avx2(src, dst, width);
        } else {
            convert_l_to_rgba_row_ssse3(src, dst, width);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_l_to_rgba_row_neon(src, dst, width);
    }
}

fn convert_l_u8_to_rgb_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_l_to_rgb_row_avx2(src, dst, width);
        } else {
            convert_l_to_rgb_row_ssse3(src, dst, width);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_l_to_rgb_row_neon(src, dst, width);
    }
}

fn convert_la_u8_to_rgba_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_la_to_rgba_row_avx2(src, dst, width);
        } else {
            convert_la_to_rgba_row_ssse3(src, dst, width);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_la_to_rgba_row_neon(src, dst, width);
    }
}

fn convert_rgba_u8_to_la_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_rgba_to_la_row_avx2(src, dst, width);
        } else {
            convert_rgba_to_la_row_ssse3(src, dst, width);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_rgba_to_la_row_neon(src, dst, width);
    }
}

// Channel-specific F32->U8 wrappers
fn convert_f32_to_u8_row_1ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_floats: &[f32] = bytemuck::cast_slice(&src[..width * 4]);
    convert_f32_to_u8_row(src_floats, &mut dst[..width]);
}

fn convert_f32_to_u8_row_2ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_floats: &[f32] = bytemuck::cast_slice(&src[..width * 8]);
    convert_f32_to_u8_row(src_floats, &mut dst[..width * 2]);
}

fn convert_f32_to_u8_row_3ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_floats: &[f32] = bytemuck::cast_slice(&src[..width * 12]);
    convert_f32_to_u8_row(src_floats, &mut dst[..width * 3]);
}

fn convert_f32_to_u8_row_4ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_floats: &[f32] = bytemuck::cast_slice(&src[..width * 16]);
    convert_f32_to_u8_row(src_floats, &mut dst[..width * 4]);
}

// Channel-specific U8<->U16 wrappers
fn convert_u8_to_u16_row_1ch(src: &[u8], dst: &mut [u8], width: usize) {
    let dst_words: &mut [u16] = bytemuck::cast_slice_mut(&mut dst[..width * 2]);
    convert_u8_to_u16_row(&src[..width], dst_words);
}

fn convert_u8_to_u16_row_2ch(src: &[u8], dst: &mut [u8], width: usize) {
    let dst_words: &mut [u16] = bytemuck::cast_slice_mut(&mut dst[..width * 4]);
    convert_u8_to_u16_row(&src[..width * 2], dst_words);
}

fn convert_u8_to_u16_row_3ch(src: &[u8], dst: &mut [u8], width: usize) {
    let dst_words: &mut [u16] = bytemuck::cast_slice_mut(&mut dst[..width * 6]);
    convert_u8_to_u16_row(&src[..width * 3], dst_words);
}

fn convert_u8_to_u16_row_4ch(src: &[u8], dst: &mut [u8], width: usize) {
    let dst_words: &mut [u16] = bytemuck::cast_slice_mut(&mut dst[..width * 8]);
    convert_u8_to_u16_row(&src[..width * 4], dst_words);
}

fn convert_u16_to_u8_row_1ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_words: &[u16] = bytemuck::cast_slice(&src[..width * 2]);
    convert_u16_to_u8_row(src_words, &mut dst[..width]);
}

fn convert_u16_to_u8_row_2ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_words: &[u16] = bytemuck::cast_slice(&src[..width * 4]);
    convert_u16_to_u8_row(src_words, &mut dst[..width * 2]);
}

fn convert_u16_to_u8_row_3ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_words: &[u16] = bytemuck::cast_slice(&src[..width * 6]);
    convert_u16_to_u8_row(src_words, &mut dst[..width * 3]);
}

fn convert_u16_to_u8_row_4ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_words: &[u16] = bytemuck::cast_slice(&src[..width * 8]);
    convert_u16_to_u8_row(src_words, &mut dst[..width * 4]);
}

// Channel-specific U16<->F32 wrappers
fn convert_u16_to_f32_row_1ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_words: &[u16] = bytemuck::cast_slice(&src[..width * 2]);
    let dst_floats: &mut [f32] = bytemuck::cast_slice_mut(&mut dst[..width * 4]);
    convert_u16_to_f32_row(src_words, dst_floats);
}

fn convert_u16_to_f32_row_2ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_words: &[u16] = bytemuck::cast_slice(&src[..width * 4]);
    let dst_floats: &mut [f32] = bytemuck::cast_slice_mut(&mut dst[..width * 8]);
    convert_u16_to_f32_row(src_words, dst_floats);
}

fn convert_f32_to_u16_row_1ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_floats: &[f32] = bytemuck::cast_slice(&src[..width * 4]);
    let dst_words: &mut [u16] = bytemuck::cast_slice_mut(&mut dst[..width * 2]);
    convert_f32_to_u16_row(src_floats, dst_words);
}

fn convert_f32_to_u16_row_2ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_floats: &[f32] = bytemuck::cast_slice(&src[..width * 8]);
    let dst_words: &mut [u16] = bytemuck::cast_slice_mut(&mut dst[..width * 4]);
    convert_f32_to_u16_row(src_floats, dst_words);
}

// =============================================================================
// Core SIMD row implementations
// =============================================================================

fn convert_f32_to_u8_row(src: &[f32], dst: &mut [u8]) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_f32_to_u8_row_avx2(src, dst);
        } else {
            convert_f32_to_u8_row_sse2(src, dst);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_f32_to_u8_row_neon(src, dst);
    }
}

fn convert_u8_to_u16_row(src: &[u8], dst: &mut [u16]) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_u8_to_u16_row_avx2(src, dst);
        } else {
            convert_u8_to_u16_row_sse2(src, dst);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_u8_to_u16_row_neon(src, dst);
    }
}

fn convert_u16_to_u8_row(src: &[u16], dst: &mut [u8]) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_u16_to_u8_row_avx2(src, dst);
        } else {
            convert_u16_to_u8_row_sse2(src, dst);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_u16_to_u8_row_neon(src, dst);
    }
}

fn convert_u16_to_f32_row(src: &[u16], dst: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_u16_to_f32_row_avx2(src, dst);
        } else {
            convert_u16_to_f32_row_sse2(src, dst);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_u16_to_f32_row_neon(src, dst);
    }
}

fn convert_f32_to_u16_row(src: &[f32], dst: &mut [u16]) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if is_x86_feature_detected!("avx2") {
            convert_f32_to_u16_row_avx2(src, dst);
        } else {
            convert_f32_to_u16_row_sse2(src, dst);
        }
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        convert_f32_to_u16_row_neon(src, dst);
    }
}

// =============================================================================
// x86_64 SIMD implementations
// =============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn convert_rgba_to_rgb_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
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
unsafe fn convert_rgb_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
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
unsafe fn convert_rgba_to_l_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
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
unsafe fn convert_rgb_to_l_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
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
unsafe fn convert_l_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
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
unsafe fn convert_l_to_rgb_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
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
unsafe fn convert_la_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
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
unsafe fn convert_rgba_to_la_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
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

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn convert_f32_to_u8_row_sse2(src: &[f32], dst: &mut [u8]) {
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
unsafe fn convert_u8_to_u16_row_sse2(src: &[u8], dst: &mut [u16]) {
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
unsafe fn convert_u16_to_u8_row_sse2(src: &[u16], dst: &mut [u8]) {
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
unsafe fn convert_u16_to_f32_row_sse2(src: &[u16], dst: &mut [f32]) {
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
unsafe fn convert_f32_to_u16_row_sse2(src: &[f32], dst: &mut [u16]) {
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

// =============================================================================
// AVX2 implementations (x86_64)
// =============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_rgba_to_rgb_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 32;
    let remainder = width % 32;

    // Process 32 pixels at a time (128 bytes in, 96 bytes out)
    let shuffle = _mm256_setr_epi8(
        0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1, 0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13,
        14, -1, -1, -1, -1,
    );

    for i in 0..simd_width {
        let src_offset = i * 128;
        let dst_offset = i * 96;

        // Load 128 bytes (32 RGBA pixels)
        let rgba0 = _mm256_loadu_si256(src_ptr.add(src_offset) as *const __m256i);
        let rgba1 = _mm256_loadu_si256(src_ptr.add(src_offset + 32) as *const __m256i);
        let rgba2 = _mm256_loadu_si256(src_ptr.add(src_offset + 64) as *const __m256i);
        let rgba3 = _mm256_loadu_si256(src_ptr.add(src_offset + 96) as *const __m256i);

        // Shuffle each 256-bit register to get RGB (24 bytes valid per 32-byte register)
        let rgb0 = _mm256_shuffle_epi8(rgba0, shuffle);
        let rgb1 = _mm256_shuffle_epi8(rgba1, shuffle);
        let rgb2 = _mm256_shuffle_epi8(rgba2, shuffle);
        let rgb3 = _mm256_shuffle_epi8(rgba3, shuffle);

        // Extract 128-bit lanes and pack properly
        let rgb0_lo = _mm256_castsi256_si128(rgb0);
        let rgb0_hi = _mm256_extracti128_si256(rgb0, 1);
        let rgb1_lo = _mm256_castsi256_si128(rgb1);
        let rgb1_hi = _mm256_extracti128_si256(rgb1, 1);
        let rgb2_lo = _mm256_castsi256_si128(rgb2);
        let rgb2_hi = _mm256_extracti128_si256(rgb2, 1);
        let rgb3_lo = _mm256_castsi256_si128(rgb3);
        let rgb3_hi = _mm256_extracti128_si256(rgb3, 1);

        // Pack into output (complex interleaving)
        // Each shuffled lane has 12 valid bytes at positions 0-11

        // First output: rgb0_lo[0-11] + rgb0_hi[0-3] -> 16 bytes
        let out0 = _mm_or_si128(rgb0_lo, _mm_slli_si128(rgb0_hi, 12));
        // Second: rgb0_hi[4-11] + rgb1_lo[0-7] -> 16 bytes
        let out1 = _mm_or_si128(_mm_srli_si128(rgb0_hi, 4), _mm_slli_si128(rgb1_lo, 8));
        // Third: rgb1_lo[8-11] + rgb1_hi[0-11] -> 16 bytes
        let out2 = _mm_or_si128(_mm_srli_si128(rgb1_lo, 8), _mm_slli_si128(rgb1_hi, 4));

        // Continue for remaining 48 bytes
        let out3 = _mm_or_si128(rgb2_lo, _mm_slli_si128(rgb2_hi, 12));
        let out4 = _mm_or_si128(_mm_srli_si128(rgb2_hi, 4), _mm_slli_si128(rgb3_lo, 8));
        let out5 = _mm_or_si128(_mm_srli_si128(rgb3_lo, 8), _mm_slli_si128(rgb3_hi, 4));

        _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, out0);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, out1);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 32) as *mut __m128i, out2);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 48) as *mut __m128i, out3);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 64) as *mut __m128i, out4);
        _mm_storeu_si128(dst_ptr.add(dst_offset + 80) as *mut __m128i, out5);
    }

    // Handle remainder with SSSE3
    if remainder > 0 {
        convert_rgba_to_rgb_row_ssse3(
            &src[simd_width * 128..],
            &mut dst[simd_width * 96..],
            remainder,
        );
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_rgb_to_rgba_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // For simplicity, fall back to SSSE3 for this operation
    // AVX2 doesn't provide significant benefit for RGB->RGBA due to the complex packing
    convert_rgb_to_rgba_row_ssse3(src, dst, width);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_rgba_to_l_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3 - AVX2 luminance conversion is complex
    convert_rgba_to_l_row_ssse3(src, dst, width);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_rgb_to_l_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3
    convert_rgb_to_l_row_ssse3(src, dst, width);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_l_to_rgba_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3 for simplicity
    convert_l_to_rgba_row_ssse3(src, dst, width);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_l_to_rgb_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3
    convert_l_to_rgb_row_ssse3(src, dst, width);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_la_to_rgba_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3
    convert_la_to_rgba_row_ssse3(src, dst, width);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_rgba_to_la_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    // Fall back to SSSE3
    convert_rgba_to_la_row_ssse3(src, dst, width);
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_f32_to_u8_row_avx2(src: &[f32], dst: &mut [u8]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 32;
    let remainder = len % 32;

    let scale = _mm256_set1_ps(255.0);
    let zero_f = _mm256_setzero_ps();
    let max_f = _mm256_set1_ps(255.0);

    // Permutation to fix the order after packing:
    // After packs_epi32 and packus_epi16, we have (as 32-bit dwords):
    // [0-3, 8-11, 16-19, 24-27, 4-7, 12-15, 20-23, 28-31]
    // We need: [0-3, 4-7, 8-11, 12-15, 16-19, 20-23, 24-27, 28-31]
    // So permutation is: 0, 4, 1, 5, 2, 6, 3, 7
    let perm = _mm256_setr_epi32(0, 4, 1, 5, 2, 6, 3, 7);

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 32;

        let f0 = _mm256_loadu_ps(src.as_ptr().add(src_offset));
        let f1 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 8));
        let f2 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 16));
        let f3 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 24));

        let scaled0 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f0, scale), zero_f), max_f);
        let scaled1 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f1, scale), zero_f), max_f);
        let scaled2 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f2, scale), zero_f), max_f);
        let scaled3 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f3, scale), zero_f), max_f);

        let i0 = _mm256_cvtps_epi32(scaled0);
        let i1 = _mm256_cvtps_epi32(scaled1);
        let i2 = _mm256_cvtps_epi32(scaled2);
        let i3 = _mm256_cvtps_epi32(scaled3);

        let words_0 = _mm256_packs_epi32(i0, i1);
        let words_1 = _mm256_packs_epi32(i2, i3);
        let bytes = _mm256_packus_epi16(words_0, words_1);

        // Permute 32-bit dwords to get correct sequential order
        let bytes = _mm256_permutevar8x32_epi32(bytes, perm);

        _mm256_storeu_si256(dst.as_mut_ptr().add(dst_offset) as *mut __m256i, bytes);
    }

    // Handle remainder with SSE2
    if remainder > 0 {
        convert_f32_to_u8_row_sse2(&src[simd_width * 32..], &mut dst[simd_width * 32..]);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_u8_to_u16_row_avx2(src: &[u8], dst: &mut [u16]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 32;
    let remainder = len % 32;

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 32;

        let bytes = _mm256_loadu_si256(src.as_ptr().add(src_offset) as *const __m256i);

        let bytes_lo = _mm256_castsi256_si128(bytes);
        let bytes_hi = _mm256_extracti128_si256(bytes, 1);

        let zero = _mm_setzero_si128();
        let words_0 = _mm_unpacklo_epi8(bytes_lo, zero);
        let words_1 = _mm_unpackhi_epi8(bytes_lo, zero);
        let words_2 = _mm_unpacklo_epi8(bytes_hi, zero);
        let words_3 = _mm_unpackhi_epi8(bytes_hi, zero);

        let scaled_0 = _mm_or_si128(words_0, _mm_slli_epi16(words_0, 8));
        let scaled_1 = _mm_or_si128(words_1, _mm_slli_epi16(words_1, 8));
        let scaled_2 = _mm_or_si128(words_2, _mm_slli_epi16(words_2, 8));
        let scaled_3 = _mm_or_si128(words_3, _mm_slli_epi16(words_3, 8));

        _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, scaled_0);
        _mm_storeu_si128(
            dst.as_mut_ptr().add(dst_offset + 8) as *mut __m128i,
            scaled_1,
        );
        _mm_storeu_si128(
            dst.as_mut_ptr().add(dst_offset + 16) as *mut __m128i,
            scaled_2,
        );
        _mm_storeu_si128(
            dst.as_mut_ptr().add(dst_offset + 24) as *mut __m128i,
            scaled_3,
        );
    }

    if remainder > 0 {
        convert_u8_to_u16_row_sse2(&src[simd_width * 32..], &mut dst[simd_width * 32..]);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_u16_to_u8_row_avx2(src: &[u16], dst: &mut [u8]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 32;
    let remainder = len % 32;

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 32;

        let words_0 = _mm256_loadu_si256(src.as_ptr().add(src_offset) as *const __m256i);
        let words_1 = _mm256_loadu_si256(src.as_ptr().add(src_offset + 16) as *const __m256i);

        let shifted_0 = _mm256_srli_epi16(words_0, 8);
        let shifted_1 = _mm256_srli_epi16(words_1, 8);

        let bytes = _mm256_packus_epi16(shifted_0, shifted_1);
        let bytes = _mm256_permute4x64_epi64(bytes, 0b11011000);

        _mm256_storeu_si256(dst.as_mut_ptr().add(dst_offset) as *mut __m256i, bytes);
    }

    if remainder > 0 {
        convert_u16_to_u8_row_sse2(&src[simd_width * 32..], &mut dst[simd_width * 32..]);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_u16_to_f32_row_avx2(src: &[u16], dst: &mut [f32]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    let scale = _mm256_set1_ps(1.0 / 65535.0);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let words = _mm256_loadu_si256(src.as_ptr().add(src_offset) as *const __m256i);

        let words_lo = _mm256_castsi256_si128(words);
        let words_hi = _mm256_extracti128_si256(words, 1);

        let dwords_0 = _mm256_cvtepu16_epi32(words_lo);
        let dwords_1 = _mm256_cvtepu16_epi32(words_hi);

        let floats_0 = _mm256_mul_ps(_mm256_cvtepi32_ps(dwords_0), scale);
        let floats_1 = _mm256_mul_ps(_mm256_cvtepi32_ps(dwords_1), scale);

        _mm256_storeu_ps(dst.as_mut_ptr().add(dst_offset), floats_0);
        _mm256_storeu_ps(dst.as_mut_ptr().add(dst_offset + 8), floats_1);
    }

    if remainder > 0 {
        convert_u16_to_f32_row_sse2(&src[simd_width * 16..], &mut dst[simd_width * 16..]);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_f32_to_u16_row_avx2(src: &[f32], dst: &mut [u16]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    let scale = _mm256_set1_ps(65535.0);
    let zero_f = _mm256_setzero_ps();
    let max_f = _mm256_set1_ps(65535.0);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let f0 = _mm256_loadu_ps(src.as_ptr().add(src_offset));
        let f1 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 8));

        let scaled0 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f0, scale), zero_f), max_f);
        let scaled1 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f1, scale), zero_f), max_f);

        let i0 = _mm256_cvtps_epi32(scaled0);
        let i1 = _mm256_cvtps_epi32(scaled1);

        // Pack to 16-bit
        let words = _mm256_packus_epi32(i0, i1);
        let words = _mm256_permute4x64_epi64(words, 0b11011000);

        _mm256_storeu_si256(dst.as_mut_ptr().add(dst_offset) as *mut __m256i, words);
    }

    if remainder > 0 {
        convert_f32_to_u16_row_sse2(&src[simd_width * 16..], &mut dst[simd_width * 16..]);
    }
}

// =============================================================================
// NEON implementations (aarch64)
// =============================================================================

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgba_to_rgb_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    for i in 0..simd_width {
        let src_offset = i * 64;
        let dst_offset = i * 48;

        // Load 16 RGBA pixels deinterleaved
        let rgba = vld4q_u8(src_ptr.add(src_offset));

        // Store as RGB (interleaved)
        let rgb = uint8x16x3_t(rgba.0, rgba.1, rgba.2);
        vst3q_u8(dst_ptr.add(dst_offset), rgb);
    }

    let src_rem = &src[simd_width * 64..];
    let dst_rem = &mut dst[simd_width * 48..];
    for i in 0..remainder {
        dst_rem[i * 3] = src_rem[i * 4];
        dst_rem[i * 3 + 1] = src_rem[i * 4 + 1];
        dst_rem[i * 3 + 2] = src_rem[i * 4 + 2];
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgb_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let alpha = vdupq_n_u8(255);

    for i in 0..simd_width {
        let src_offset = i * 48;
        let dst_offset = i * 64;

        // Load 16 RGB pixels deinterleaved
        let rgb = vld3q_u8(src_ptr.add(src_offset));

        // Store as RGBA
        let rgba = uint8x16x4_t(rgb.0, rgb.1, rgb.2, alpha);
        vst4q_u8(dst_ptr.add(dst_offset), rgba);
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

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgba_to_l_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let r_w = vdupq_n_u16(54);
    let g_w = vdupq_n_u16(183);
    let b_w = vdupq_n_u16(19);

    for i in 0..simd_width {
        let src_offset = i * 64;
        let dst_offset = i * 16;

        let rgba = vld4q_u8(src_ptr.add(src_offset));
        let r = rgba.0;
        let g = rgba.1;
        let b = rgba.2;

        let r_lo = vmovl_u8(vget_low_u8(r));
        let r_hi = vmovl_u8(vget_high_u8(r));
        let g_lo = vmovl_u8(vget_low_u8(g));
        let g_hi = vmovl_u8(vget_high_u8(g));
        let b_lo = vmovl_u8(vget_low_u8(b));
        let b_hi = vmovl_u8(vget_high_u8(b));

        let sum_lo = vaddq_u16(
            vaddq_u16(vmulq_u16(r_lo, r_w), vmulq_u16(g_lo, g_w)),
            vmulq_u16(b_lo, b_w),
        );
        let sum_hi = vaddq_u16(
            vaddq_u16(vmulq_u16(r_hi, r_w), vmulq_u16(g_hi, g_w)),
            vmulq_u16(b_hi, b_w),
        );

        let lum_lo = vshrn_n_u16(sum_lo, 8);
        let lum_hi = vshrn_n_u16(sum_hi, 8);
        let lum = vcombine_u8(lum_lo, lum_hi);

        vst1q_u8(dst_ptr.add(dst_offset), lum);
    }

    let src_rem = &src[simd_width * 64..];
    let dst_rem = &mut dst[simd_width * 16..];
    for i in 0..remainder {
        let r = src_rem[i * 4] as u32;
        let g = src_rem[i * 4 + 1] as u32;
        let b = src_rem[i * 4 + 2] as u32;
        dst_rem[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgb_to_l_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let r_w = vdupq_n_u16(54);
    let g_w = vdupq_n_u16(183);
    let b_w = vdupq_n_u16(19);

    for i in 0..simd_width {
        let src_offset = i * 48;
        let dst_offset = i * 16;

        let rgb = vld3q_u8(src_ptr.add(src_offset));
        let r = rgb.0;
        let g = rgb.1;
        let b = rgb.2;

        let r_lo = vmovl_u8(vget_low_u8(r));
        let r_hi = vmovl_u8(vget_high_u8(r));
        let g_lo = vmovl_u8(vget_low_u8(g));
        let g_hi = vmovl_u8(vget_high_u8(g));
        let b_lo = vmovl_u8(vget_low_u8(b));
        let b_hi = vmovl_u8(vget_high_u8(b));

        let sum_lo = vaddq_u16(
            vaddq_u16(vmulq_u16(r_lo, r_w), vmulq_u16(g_lo, g_w)),
            vmulq_u16(b_lo, b_w),
        );
        let sum_hi = vaddq_u16(
            vaddq_u16(vmulq_u16(r_hi, r_w), vmulq_u16(g_hi, g_w)),
            vmulq_u16(b_hi, b_w),
        );

        let lum_lo = vshrn_n_u16(sum_lo, 8);
        let lum_hi = vshrn_n_u16(sum_hi, 8);
        let lum = vcombine_u8(lum_lo, lum_hi);

        vst1q_u8(dst_ptr.add(dst_offset), lum);
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

#[cfg(target_arch = "aarch64")]
unsafe fn convert_l_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let alpha = vdupq_n_u8(255);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 64;

        let l = vld1q_u8(src_ptr.add(src_offset));
        let rgba = uint8x16x4_t(l, l, l, alpha);
        vst4q_u8(dst_ptr.add(dst_offset), rgba);
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

#[cfg(target_arch = "aarch64")]
unsafe fn convert_l_to_rgb_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 48;

        let l = vld1q_u8(src_ptr.add(src_offset));
        let rgb = uint8x16x3_t(l, l, l);
        vst3q_u8(dst_ptr.add(dst_offset), rgb);
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

#[cfg(target_arch = "aarch64")]
unsafe fn convert_la_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 8;
    let remainder = width % 8;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 32;

        let la = vld2q_u8(src_ptr.add(src_offset));
        let l = la.0;
        let a = la.1;

        let rgba = uint8x16x4_t(l, l, l, a);
        vst4q_u8(dst_ptr.add(dst_offset), rgba);
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

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgba_to_la_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();
    let simd_width = width / 16;
    let remainder = width % 16;

    let r_w = vdupq_n_u16(54);
    let g_w = vdupq_n_u16(183);
    let b_w = vdupq_n_u16(19);

    for i in 0..simd_width {
        let src_offset = i * 64;
        let dst_offset = i * 32;

        let rgba = vld4q_u8(src_ptr.add(src_offset));
        let r = rgba.0;
        let g = rgba.1;
        let b = rgba.2;
        let a = rgba.3;

        let r_lo = vmovl_u8(vget_low_u8(r));
        let r_hi = vmovl_u8(vget_high_u8(r));
        let g_lo = vmovl_u8(vget_low_u8(g));
        let g_hi = vmovl_u8(vget_high_u8(g));
        let b_lo = vmovl_u8(vget_low_u8(b));
        let b_hi = vmovl_u8(vget_high_u8(b));

        let sum_lo = vaddq_u16(
            vaddq_u16(vmulq_u16(r_lo, r_w), vmulq_u16(g_lo, g_w)),
            vmulq_u16(b_lo, b_w),
        );
        let sum_hi = vaddq_u16(
            vaddq_u16(vmulq_u16(r_hi, r_w), vmulq_u16(g_hi, g_w)),
            vmulq_u16(b_hi, b_w),
        );

        let lum_lo = vshrn_n_u16(sum_lo, 8);
        let lum_hi = vshrn_n_u16(sum_hi, 8);
        let lum = vcombine_u8(lum_lo, lum_hi);

        let la = uint8x16x2_t(lum, a);
        vst2q_u8(dst_ptr.add(dst_offset), la);
    }

    let src_rem = &src[simd_width * 64..];
    let dst_rem = &mut dst[simd_width * 32..];
    for i in 0..remainder {
        let r = src_rem[i * 4] as u32;
        let g = src_rem[i * 4 + 1] as u32;
        let b = src_rem[i * 4 + 2] as u32;
        let a = src_rem[i * 4 + 3];
        dst_rem[i * 2] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
        dst_rem[i * 2 + 1] = a;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_f32_to_u8_row_neon(src: &[f32], dst: &mut [u8]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    let scale = vdupq_n_f32(255.0);
    let zero = vdupq_n_f32(0.0);
    let max = vdupq_n_f32(255.0);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let f0 = vld1q_f32(src.as_ptr().add(src_offset));
        let f1 = vld1q_f32(src.as_ptr().add(src_offset + 4));
        let f2 = vld1q_f32(src.as_ptr().add(src_offset + 8));
        let f3 = vld1q_f32(src.as_ptr().add(src_offset + 12));

        let scaled0 = vminq_f32(vmaxq_f32(vmulq_f32(f0, scale), zero), max);
        let scaled1 = vminq_f32(vmaxq_f32(vmulq_f32(f1, scale), zero), max);
        let scaled2 = vminq_f32(vmaxq_f32(vmulq_f32(f2, scale), zero), max);
        let scaled3 = vminq_f32(vmaxq_f32(vmulq_f32(f3, scale), zero), max);

        let u0 = vcvtq_u32_f32(scaled0);
        let u1 = vcvtq_u32_f32(scaled1);
        let u2 = vcvtq_u32_f32(scaled2);
        let u3 = vcvtq_u32_f32(scaled3);

        let words_lo = vcombine_u16(vmovn_u32(u0), vmovn_u32(u1));
        let words_hi = vcombine_u16(vmovn_u32(u2), vmovn_u32(u3));

        let bytes = vcombine_u8(vmovn_u16(words_lo), vmovn_u16(words_hi));

        vst1q_u8(dst.as_mut_ptr().add(dst_offset), bytes);
    }

    for i in 0..remainder {
        let val = (src[simd_width * 16 + i] * 255.0).clamp(0.0, 255.0) as u8;
        dst[simd_width * 16 + i] = val;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_u8_to_u16_row_neon(src: &[u8], dst: &mut [u16]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let bytes = vld1q_u8(src.as_ptr().add(src_offset));

        let words_lo = vmovl_u8(vget_low_u8(bytes));
        let words_hi = vmovl_u8(vget_high_u8(bytes));

        let scaled_lo = vorrq_u16(words_lo, vshlq_n_u16(words_lo, 8));
        let scaled_hi = vorrq_u16(words_hi, vshlq_n_u16(words_hi, 8));

        vst1q_u16(dst.as_mut_ptr().add(dst_offset), scaled_lo);
        vst1q_u16(dst.as_mut_ptr().add(dst_offset + 8), scaled_hi);
    }

    for i in 0..remainder {
        dst[simd_width * 16 + i] = (src[simd_width * 16 + i] as u16) * 257;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_u16_to_u8_row_neon(src: &[u16], dst: &mut [u8]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        let words_lo = vld1q_u16(src.as_ptr().add(src_offset));
        let words_hi = vld1q_u16(src.as_ptr().add(src_offset + 8));

        let shifted_lo = vshrq_n_u16(words_lo, 8);
        let shifted_hi = vshrq_n_u16(words_hi, 8);

        let bytes = vcombine_u8(vmovn_u16(shifted_lo), vmovn_u16(shifted_hi));

        vst1q_u8(dst.as_mut_ptr().add(dst_offset), bytes);
    }

    for i in 0..remainder {
        dst[simd_width * 16 + i] = (src[simd_width * 16 + i] / 257) as u8;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_u16_to_f32_row_neon(src: &[u16], dst: &mut [f32]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 8;
    let remainder = len % 8;

    let scale = vdupq_n_f32(1.0 / 65535.0);

    for i in 0..simd_width {
        let src_offset = i * 8;
        let dst_offset = i * 8;

        let words = vld1q_u16(src.as_ptr().add(src_offset));

        let dwords_lo = vmovl_u16(vget_low_u16(words));
        let dwords_hi = vmovl_u16(vget_high_u16(words));

        let floats_lo = vmulq_f32(vcvtq_f32_u32(dwords_lo), scale);
        let floats_hi = vmulq_f32(vcvtq_f32_u32(dwords_hi), scale);

        vst1q_f32(dst.as_mut_ptr().add(dst_offset), floats_lo);
        vst1q_f32(dst.as_mut_ptr().add(dst_offset + 4), floats_hi);
    }

    for i in 0..remainder {
        dst[simd_width * 8 + i] = src[simd_width * 8 + i] as f32 / 65535.0;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_f32_to_u16_row_neon(src: &[f32], dst: &mut [u16]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 8;
    let remainder = len % 8;

    let scale = vdupq_n_f32(65535.0);
    let zero = vdupq_n_f32(0.0);
    let max = vdupq_n_f32(65535.0);

    for i in 0..simd_width {
        let src_offset = i * 8;
        let dst_offset = i * 8;

        let f0 = vld1q_f32(src.as_ptr().add(src_offset));
        let f1 = vld1q_f32(src.as_ptr().add(src_offset + 4));

        let scaled0 = vminq_f32(vmaxq_f32(vmulq_f32(f0, scale), zero), max);
        let scaled1 = vminq_f32(vmaxq_f32(vmulq_f32(f1, scale), zero), max);

        let u0 = vcvtq_u32_f32(scaled0);
        let u1 = vcvtq_u32_f32(scaled1);

        let words = vcombine_u16(vmovn_u32(u0), vmovn_u32(u1));

        vst1q_u16(dst.as_mut_ptr().add(dst_offset), words);
    }

    for i in 0..remainder {
        dst[simd_width * 8 + i] = (src[simd_width * 8 + i] * 65535.0).clamp(0.0, 65535.0) as u16;
    }
}
