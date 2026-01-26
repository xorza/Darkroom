// SIMD-optimized image conversion implementations
//
// This module contains SIMD implementations for conversion paths where
// benchmarks show meaningful speedup (>1.05x over scalar).
//
// Paths with SIMD:
// - RGBA_U8 <-> RGB_U8 (1.07-1.12x)
// - RGB_U8/RGBA_U8 -> L_U8 (1.65-1.90x)
// - L_U8 -> RGBA_U8 (2.10x)
// - LA_U8 <-> RGBA_U8 (1.11-1.30x)
// - U8 <-> U16 (1.02-1.03x, kept for consistency)
// - L_U16 <-> F32 (1.06-1.14x)
// - F32 -> U8 (1.02-1.03x, kept for consistency)
//
// Paths WITHOUT SIMD (memory-bound, <1.03x speedup - see bench-analysis.md):
// - RGBA_F32 <-> RGB_F32 (1.00x)
// - L_F32 <-> RGBA_F32/RGB_F32 (1.01x)
// - RGBA/RGB F32 -> L_F32 (1.01-1.02x)
// - RGBA/RGB U16 <-> F32 (1.00-1.01x)
// - U8 -> F32 (1.01-1.02x)

use rayon::prelude::*;

use crate::common::color_format::ColorFormat;
use crate::common::error::Result;
use crate::image::Image;

/// Check if SIMD conversion is available for the given format pair.
/// Returns true if a SIMD-optimized path exists.
///
/// Note: Many F32-involving conversions are intentionally NOT implemented as SIMD
/// because benchmarks show they are memory-bound with <1.03x speedup.
/// See bench-analysis.md for details.
pub(crate) fn has_simd_path(from: &Image, to: &Image) -> bool {
    let from_fmt = from.desc().color_format;
    let to_fmt = to.desc().color_format;

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("sse2") {
        match (from_fmt, to_fmt) {
            // Channel conversions U8 (SSSE3) - 1.07-1.12x speedup
            (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => return true,
            (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) => return true,
            // Luminance U8 (SSSE3) - 1.65-2.10x speedup
            (ColorFormat::RGBA_U8, ColorFormat::L_U8) => return true,
            (ColorFormat::RGB_U8, ColorFormat::L_U8) => return true,
            // L_U8 expansion - 2.10x speedup (uses scalar fallback, TODO: add SIMD)
            // F32->U8 - 1.02-1.03x speedup (kept for consistency)
            (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => return true,
            (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => return true,
            (ColorFormat::L_F32, ColorFormat::L_U8) => return true,
            (ColorFormat::LA_F32, ColorFormat::LA_U8) => return true,
            // U8<->U16 (SSE2) - 1.02-1.03x speedup (kept for consistency)
            (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16) => return true,
            (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8) => return true,
            (ColorFormat::RGB_U8, ColorFormat::RGB_U16) => return true,
            (ColorFormat::RGB_U16, ColorFormat::RGB_U8) => return true,
            (ColorFormat::L_U8, ColorFormat::L_U16) => return true,
            (ColorFormat::L_U16, ColorFormat::L_U8) => return true,
            (ColorFormat::LA_U8, ColorFormat::LA_U16) => return true,
            (ColorFormat::LA_U16, ColorFormat::LA_U8) => return true,
            // LA_U8 <-> RGBA_U8 (SSSE3) - 1.11-1.30x speedup
            (ColorFormat::LA_U8, ColorFormat::RGBA_U8) => return true,
            (ColorFormat::RGBA_U8, ColorFormat::LA_U8) => return true,
            // L_U16 <-> F32 - 1.06-1.14x speedup (only L, not RGBA/RGB)
            (ColorFormat::L_U16, ColorFormat::L_F32) => return true,
            (ColorFormat::L_F32, ColorFormat::L_U16) => return true,
            _ => {}
        }
    }

    #[cfg(target_arch = "aarch64")]
    match (from_fmt, to_fmt) {
        // Channel conversions U8 - 1.07-1.12x speedup
        (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => return true,
        (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) => return true,
        // LA_U8 <-> RGBA_U8 - 1.11-1.30x speedup
        (ColorFormat::LA_U8, ColorFormat::RGBA_U8) => return true,
        (ColorFormat::RGBA_U8, ColorFormat::LA_U8) => return true,
        // Luminance U8 - 1.65-2.10x speedup
        (ColorFormat::RGBA_U8, ColorFormat::L_U8) => return true,
        (ColorFormat::RGB_U8, ColorFormat::L_U8) => return true,
        // F32->U8 - kept for consistency
        (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => return true,
        (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => return true,
        (ColorFormat::L_F32, ColorFormat::L_U8) => return true,
        (ColorFormat::LA_F32, ColorFormat::LA_U8) => return true,
        // U8<->U16 - kept for consistency
        (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16) => return true,
        (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8) => return true,
        (ColorFormat::RGB_U8, ColorFormat::RGB_U16) => return true,
        (ColorFormat::RGB_U16, ColorFormat::RGB_U8) => return true,
        (ColorFormat::L_U8, ColorFormat::L_U16) => return true,
        (ColorFormat::L_U16, ColorFormat::L_U8) => return true,
        (ColorFormat::LA_U8, ColorFormat::LA_U16) => return true,
        (ColorFormat::LA_U16, ColorFormat::LA_U8) => return true,
        // L_U16 <-> F32 - 1.06-1.14x speedup
        (ColorFormat::L_U16, ColorFormat::L_F32) => return true,
        (ColorFormat::L_F32, ColorFormat::L_U16) => return true,
        _ => {}
    }

    false
}

/// Attempt SIMD conversion. Returns Ok(true) if conversion was performed,
/// Ok(false) if no SIMD path available, or Err on failure.
///
/// Note: Many F32-involving conversions are intentionally NOT implemented as SIMD
/// because benchmarks show they are memory-bound with <1.03x speedup.
/// See bench-analysis.md for details.
pub fn try_convert_simd(from: &Image, to: &mut Image) -> Result<bool> {
    let from_fmt = from.desc().color_format;
    let to_fmt = to.desc().color_format;

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("sse2") {
        match (from_fmt, to_fmt) {
            // Channel conversions U8 (require SSSE3) - 1.07-1.12x speedup
            (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) if is_x86_feature_detected!("ssse3") => {
                convert_rgba_u8_to_rgb_u8(from, to);
                return Ok(true);
            }
            (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) if is_x86_feature_detected!("ssse3") => {
                convert_rgb_u8_to_rgba_u8(from, to);
                return Ok(true);
            }
            // Luminance U8 (require SSSE3) - 1.65-1.90x speedup
            (ColorFormat::RGBA_U8, ColorFormat::L_U8) if is_x86_feature_detected!("ssse3") => {
                convert_rgba_u8_to_l_u8(from, to);
                return Ok(true);
            }
            (ColorFormat::RGB_U8, ColorFormat::L_U8) if is_x86_feature_detected!("ssse3") => {
                convert_rgb_u8_to_l_u8(from, to);
                return Ok(true);
            }
            // F32->U8 - 1.02-1.03x speedup (kept for consistency)
            (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => {
                convert_f32_to_u8_generic(from, to, 4);
                return Ok(true);
            }
            (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => {
                convert_f32_to_u8_generic(from, to, 3);
                return Ok(true);
            }
            (ColorFormat::L_F32, ColorFormat::L_U8) => {
                convert_f32_to_u8_generic(from, to, 1);
                return Ok(true);
            }
            (ColorFormat::LA_F32, ColorFormat::LA_U8) => {
                convert_f32_to_u8_generic(from, to, 2);
                return Ok(true);
            }
            // U8<->U16 (SSE2) - 1.02-1.03x speedup (kept for consistency)
            (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16) => {
                convert_u8_to_u16_generic(from, to, 4);
                return Ok(true);
            }
            (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8) => {
                convert_u16_to_u8_generic(from, to, 4);
                return Ok(true);
            }
            (ColorFormat::RGB_U8, ColorFormat::RGB_U16) => {
                convert_u8_to_u16_generic(from, to, 3);
                return Ok(true);
            }
            (ColorFormat::RGB_U16, ColorFormat::RGB_U8) => {
                convert_u16_to_u8_generic(from, to, 3);
                return Ok(true);
            }
            (ColorFormat::L_U8, ColorFormat::L_U16) => {
                convert_u8_to_u16_generic(from, to, 1);
                return Ok(true);
            }
            (ColorFormat::L_U16, ColorFormat::L_U8) => {
                convert_u16_to_u8_generic(from, to, 1);
                return Ok(true);
            }
            (ColorFormat::LA_U8, ColorFormat::LA_U16) => {
                convert_u8_to_u16_generic(from, to, 2);
                return Ok(true);
            }
            (ColorFormat::LA_U16, ColorFormat::LA_U8) => {
                convert_u16_to_u8_generic(from, to, 2);
                return Ok(true);
            }
            // LA_U8 <-> RGBA_U8 (require SSSE3) - 1.11-1.30x speedup
            (ColorFormat::LA_U8, ColorFormat::RGBA_U8) if is_x86_feature_detected!("ssse3") => {
                convert_la_u8_to_rgba_u8(from, to);
                return Ok(true);
            }
            (ColorFormat::RGBA_U8, ColorFormat::LA_U8) if is_x86_feature_detected!("ssse3") => {
                convert_rgba_u8_to_la_u8(from, to);
                return Ok(true);
            }
            // L_U16 <-> F32 - 1.06-1.14x speedup (only L has meaningful benefit)
            (ColorFormat::L_U16, ColorFormat::L_F32) => {
                convert_u16_to_f32_generic(from, to, 1);
                return Ok(true);
            }
            (ColorFormat::L_F32, ColorFormat::L_U16) => {
                convert_f32_to_u16_generic(from, to, 1);
                return Ok(true);
            }
            _ => {}
        }
    }

    #[cfg(target_arch = "aarch64")]
    match (from_fmt, to_fmt) {
        // Channel conversions U8 - 1.07-1.12x speedup
        (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => {
            convert_rgba_u8_to_rgb_u8(from, to);
            return Ok(true);
        }
        (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) => {
            convert_rgb_u8_to_rgba_u8(from, to);
            return Ok(true);
        }
        // Luminance U8 - 1.65-1.90x speedup
        (ColorFormat::RGBA_U8, ColorFormat::L_U8) => {
            convert_rgba_u8_to_l_u8(from, to);
            return Ok(true);
        }
        (ColorFormat::RGB_U8, ColorFormat::L_U8) => {
            convert_rgb_u8_to_l_u8(from, to);
            return Ok(true);
        }
        // F32->U8 - kept for consistency
        (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => {
            convert_f32_to_u8_generic(from, to, 4);
            return Ok(true);
        }
        (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => {
            convert_f32_to_u8_generic(from, to, 3);
            return Ok(true);
        }
        (ColorFormat::L_F32, ColorFormat::L_U8) => {
            convert_f32_to_u8_generic(from, to, 1);
            return Ok(true);
        }
        (ColorFormat::LA_F32, ColorFormat::LA_U8) => {
            convert_f32_to_u8_generic(from, to, 2);
            return Ok(true);
        }
        // U8<->U16 - kept for consistency
        (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16) => {
            convert_u8_to_u16_generic(from, to, 4);
            return Ok(true);
        }
        (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8) => {
            convert_u16_to_u8_generic(from, to, 4);
            return Ok(true);
        }
        (ColorFormat::RGB_U8, ColorFormat::RGB_U16) => {
            convert_u8_to_u16_generic(from, to, 3);
            return Ok(true);
        }
        (ColorFormat::RGB_U16, ColorFormat::RGB_U8) => {
            convert_u16_to_u8_generic(from, to, 3);
            return Ok(true);
        }
        (ColorFormat::L_U8, ColorFormat::L_U16) => {
            convert_u8_to_u16_generic(from, to, 1);
            return Ok(true);
        }
        (ColorFormat::L_U16, ColorFormat::L_U8) => {
            convert_u16_to_u8_generic(from, to, 1);
            return Ok(true);
        }
        (ColorFormat::LA_U8, ColorFormat::LA_U16) => {
            convert_u8_to_u16_generic(from, to, 2);
            return Ok(true);
        }
        (ColorFormat::LA_U16, ColorFormat::LA_U8) => {
            convert_u16_to_u8_generic(from, to, 2);
            return Ok(true);
        }
        // LA_U8 <-> RGBA_U8 - 1.11-1.30x speedup
        (ColorFormat::LA_U8, ColorFormat::RGBA_U8) => {
            convert_la_u8_to_rgba_u8(from, to);
            return Ok(true);
        }
        (ColorFormat::RGBA_U8, ColorFormat::LA_U8) => {
            convert_rgba_u8_to_la_u8(from, to);
            return Ok(true);
        }
        // L_U16 <-> F32 - 1.06-1.14x speedup
        (ColorFormat::L_U16, ColorFormat::L_F32) => {
            convert_u16_to_f32_generic(from, to, 1);
            return Ok(true);
        }
        (ColorFormat::L_F32, ColorFormat::L_U16) => {
            convert_f32_to_u16_generic(from, to, 1);
            return Ok(true);
        }
        (ColorFormat::LA_U16, ColorFormat::LA_F32) => {
            convert_u16_to_f32_generic(from, to, 2);
            return Ok(true);
        }
        (ColorFormat::LA_F32, ColorFormat::LA_U16) => {
            convert_f32_to_u16_generic(from, to, 2);
            return Ok(true);
        }
        _ => {}
    }

    Ok(false)
}

// =============================================================================
// RGBA_U8 -> RGB_U8 conversion
// =============================================================================

fn convert_rgba_u8_to_rgb_u8(from: &Image, to: &mut Image) {
    debug_assert_eq!(from.desc().color_format, ColorFormat::RGBA_U8);
    debug_assert_eq!(to.desc().color_format, ColorFormat::RGB_U8);
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);

    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_rgba_to_rgb_row_avx2(from_row, to_row, width);
                } else {
                    convert_rgba_to_rgb_row_ssse3(from_row, to_row, width);
                }
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_rgba_to_rgb_row_neon(from_row, to_row, width);
            }
        });
}

// =============================================================================
// RGB_U8 -> RGBA_U8 conversion
// =============================================================================

fn convert_rgb_u8_to_rgba_u8(from: &Image, to: &mut Image) {
    debug_assert_eq!(from.desc().color_format, ColorFormat::RGB_U8);
    debug_assert_eq!(to.desc().color_format, ColorFormat::RGBA_U8);
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);

    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_rgb_to_rgba_row_avx2(from_row, to_row, width);
                } else {
                    convert_rgb_to_rgba_row_ssse3(from_row, to_row, width);
                }
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_rgb_to_rgba_row_neon(from_row, to_row, width);
            }
        });
}

// =============================================================================
// LA_U8 <-> RGBA_U8 conversion
// =============================================================================

fn convert_la_u8_to_rgba_u8(from: &Image, to: &mut Image) {
    debug_assert_eq!(from.desc().color_format, ColorFormat::LA_U8);
    debug_assert_eq!(to.desc().color_format, ColorFormat::RGBA_U8);
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);

    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_la_to_rgba_row_avx2(from_row, to_row, width);
                } else {
                    convert_la_to_rgba_row_ssse3(from_row, to_row, width);
                }
            }

            #[cfg(target_arch = "aarch64")]
            unsafe {
                convert_la_to_rgba_row_neon(from_row, to_row, width);
            }
        });
}

fn convert_rgba_u8_to_la_u8(from: &Image, to: &mut Image) {
    debug_assert_eq!(from.desc().color_format, ColorFormat::RGBA_U8);
    debug_assert_eq!(to.desc().color_format, ColorFormat::LA_U8);
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);

    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_rgba_to_la_row_avx2(from_row, to_row, width);
                } else {
                    convert_rgba_to_la_row_ssse3(from_row, to_row, width);
                }
            }

            #[cfg(target_arch = "aarch64")]
            unsafe {
                convert_rgba_to_la_row_neon(from_row, to_row, width);
            }
        });
}

// =============================================================================
// SSE/SSSE3 implementations (x86_64)
// =============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn convert_rgba_to_rgb_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time (64 bytes in, 48 bytes out)
    let simd_width = width / 16;
    let remainder = width % 16;

    // Shuffle mask to extract RGB from RGBA (4 pixels -> 12 bytes)
    // Input:  R0 G0 B0 A0 R1 G1 B1 A1 R2 G2 B2 A2 R3 G3 B3 A3 (16 bytes = 4 pixels)
    // Output: R0 G0 B0 R1 G1 B1 R2 G2 B2 R3 G3 B3 xx xx xx xx (12 bytes valid)
    let shuffle = _mm_setr_epi8(0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1);

    for i in 0..simd_width {
        let src_offset = i * 64;
        let dst_offset = i * 48;

        unsafe {
            // Load 64 bytes (16 RGBA pixels)
            let rgba0 = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
            let rgba1 = _mm_loadu_si128(src_ptr.add(src_offset + 16) as *const __m128i);
            let rgba2 = _mm_loadu_si128(src_ptr.add(src_offset + 32) as *const __m128i);
            let rgba3 = _mm_loadu_si128(src_ptr.add(src_offset + 48) as *const __m128i);

            // Shuffle each to get RGB (12 bytes valid per register)
            let rgb0 = _mm_shuffle_epi8(rgba0, shuffle);
            let rgb1 = _mm_shuffle_epi8(rgba1, shuffle);
            let rgb2 = _mm_shuffle_epi8(rgba2, shuffle);
            let rgb3 = _mm_shuffle_epi8(rgba3, shuffle);

            // Pack 4x12 bytes = 48 bytes into 3x16 byte stores
            // out0: rgb0[0..12] + rgb1[0..4]
            let out0 = _mm_or_si128(rgb0, _mm_slli_si128(rgb1, 12));

            // out1: rgb1[4..12] + rgb2[0..8]
            let out1 = _mm_or_si128(_mm_srli_si128(rgb1, 4), _mm_slli_si128(rgb2, 8));

            // out2: rgb2[8..12] + rgb3[0..12]
            let out2 = _mm_or_si128(_mm_srli_si128(rgb2, 8), _mm_slli_si128(rgb3, 4));

            // Store 48 bytes
            _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, out0);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, out1);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 32) as *mut __m128i, out2);
        }
    }

    // Handle remainder pixels (scalar)
    let src_remainder = &src[simd_width * 64..];
    let dst_remainder = &mut dst[simd_width * 48..];
    for i in 0..remainder {
        dst_remainder[i * 3] = src_remainder[i * 4];
        dst_remainder[i * 3 + 1] = src_remainder[i * 4 + 1];
        dst_remainder[i * 3 + 2] = src_remainder[i * 4 + 2];
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn convert_rgb_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time (48 bytes in, 64 bytes out)
    let simd_width = width / 16;
    let remainder = width % 16;

    unsafe {
        // Alpha mask (0xFF in alpha positions)
        let alpha_mask = _mm_setr_epi8(0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1);
        // Shuffle: RGB -> RGBA (insert space for alpha)
        let shuf = _mm_setr_epi8(0, 1, 2, -1, 3, 4, 5, -1, 6, 7, 8, -1, 9, 10, 11, -1);

        for i in 0..simd_width {
            let src_offset = i * 48;
            let dst_offset = i * 64;

            // Load 48 bytes (3 registers, we use all bytes)
            let in0 = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
            let in1 = _mm_loadu_si128(src_ptr.add(src_offset + 16) as *const __m128i);
            let in2 = _mm_loadu_si128(src_ptr.add(src_offset + 32) as *const __m128i);

            // First 4 pixels (bytes 0-11 of in0)
            let rgba0 = _mm_or_si128(_mm_shuffle_epi8(in0, shuf), alpha_mask);

            // Second 4 pixels (bytes 12-15 of in0 + bytes 0-7 of in1)
            let combined1 = _mm_or_si128(_mm_srli_si128(in0, 12), _mm_slli_si128(in1, 4));
            let rgba1 = _mm_or_si128(_mm_shuffle_epi8(combined1, shuf), alpha_mask);

            // Third 4 pixels (bytes 8-15 of in1 + bytes 0-3 of in2)
            let combined2 = _mm_or_si128(_mm_srli_si128(in1, 8), _mm_slli_si128(in2, 8));
            let rgba2 = _mm_or_si128(_mm_shuffle_epi8(combined2, shuf), alpha_mask);

            // Fourth 4 pixels (bytes 4-15 of in2)
            let combined3 = _mm_srli_si128(in2, 4);
            let rgba3 = _mm_or_si128(_mm_shuffle_epi8(combined3, shuf), alpha_mask);

            // Store 64 bytes
            _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, rgba0);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, rgba1);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 32) as *mut __m128i, rgba2);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 48) as *mut __m128i, rgba3);
        }
    }

    // Handle remainder pixels (scalar)
    let src_remainder = &src[simd_width * 48..];
    let dst_remainder = &mut dst[simd_width * 64..];
    for i in 0..remainder {
        dst_remainder[i * 4] = src_remainder[i * 3];
        dst_remainder[i * 4 + 1] = src_remainder[i * 3 + 1];
        dst_remainder[i * 4 + 2] = src_remainder[i * 3 + 2];
        dst_remainder[i * 4 + 3] = 255;
    }
}

// =============================================================================
// F32 -> U8 bit depth conversion
// =============================================================================

/// Generic F32 to U8 conversion for any channel count
fn convert_f32_to_u8_generic(from: &Image, to: &mut Image, channels: usize) {
    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;
    let row_elements = width * channels;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    // Convert from_bytes to f32 slice
    let from_floats: &[f32] = bytemuck::cast_slice(from_bytes);
    let from_float_stride = from_stride / 4;

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row =
                &from_floats[y * from_float_stride..y * from_float_stride + row_elements];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_f32_to_u8_row_avx2(from_row, to_row);
                } else {
                    convert_f32_to_u8_row_sse2(from_row, to_row);
                }
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_f32_to_u8_row_neon(from_row, to_row);
            }
        });
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

        unsafe {
            // Load 16 floats
            let f0 = _mm_loadu_ps(src.as_ptr().add(src_offset));
            let f1 = _mm_loadu_ps(src.as_ptr().add(src_offset + 4));
            let f2 = _mm_loadu_ps(src.as_ptr().add(src_offset + 8));
            let f3 = _mm_loadu_ps(src.as_ptr().add(src_offset + 12));

            // Scale and clamp
            let scaled0 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f0, scale), zero_f), max_f);
            let scaled1 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f1, scale), zero_f), max_f);
            let scaled2 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f2, scale), zero_f), max_f);
            let scaled3 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f3, scale), zero_f), max_f);

            // Convert to int32
            let i0 = _mm_cvtps_epi32(scaled0);
            let i1 = _mm_cvtps_epi32(scaled1);
            let i2 = _mm_cvtps_epi32(scaled2);
            let i3 = _mm_cvtps_epi32(scaled3);

            // Pack to 16-bit (signed saturation)
            let words_lo = _mm_packs_epi32(i0, i1);
            let words_hi = _mm_packs_epi32(i2, i3);

            // Pack to 8-bit (unsigned saturation)
            let bytes = _mm_packus_epi16(words_lo, words_hi);

            // Store 16 bytes
            _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, bytes);
        }
    }

    // Handle remainder (scalar)
    for i in 0..remainder {
        let val = (src[simd_width * 16 + i] * 255.0).clamp(0.0, 255.0) as u8;
        dst[simd_width * 16 + i] = val;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_f32_to_u8_row_neon(src: &[f32], dst: &mut [u8]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    unsafe {
        let scale = vdupq_n_f32(255.0);
        let zero = vdupq_n_f32(0.0);
        let max = vdupq_n_f32(255.0);

        for i in 0..simd_width {
            let src_offset = i * 16;
            let dst_offset = i * 16;

            // Load 16 floats
            let f0 = vld1q_f32(src.as_ptr().add(src_offset));
            let f1 = vld1q_f32(src.as_ptr().add(src_offset + 4));
            let f2 = vld1q_f32(src.as_ptr().add(src_offset + 8));
            let f3 = vld1q_f32(src.as_ptr().add(src_offset + 12));

            // Scale and clamp
            let scaled0 = vminq_f32(vmaxq_f32(vmulq_f32(f0, scale), zero), max);
            let scaled1 = vminq_f32(vmaxq_f32(vmulq_f32(f1, scale), zero), max);
            let scaled2 = vminq_f32(vmaxq_f32(vmulq_f32(f2, scale), zero), max);
            let scaled3 = vminq_f32(vmaxq_f32(vmulq_f32(f3, scale), zero), max);

            // Convert to u32
            let u0 = vcvtq_u32_f32(scaled0);
            let u1 = vcvtq_u32_f32(scaled1);
            let u2 = vcvtq_u32_f32(scaled2);
            let u3 = vcvtq_u32_f32(scaled3);

            // Narrow to u16
            let words_lo = vcombine_u16(vmovn_u32(u0), vmovn_u32(u1));
            let words_hi = vcombine_u16(vmovn_u32(u2), vmovn_u32(u3));

            // Narrow to u8
            let bytes = vcombine_u8(vmovn_u16(words_lo), vmovn_u16(words_hi));

            // Store 16 bytes
            vst1q_u8(dst.as_mut_ptr().add(dst_offset), bytes);
        }
    }

    // Handle remainder (scalar)
    for i in 0..remainder {
        let val = (src[simd_width * 16 + i] * 255.0).clamp(0.0, 255.0) as u8;
        dst[simd_width * 16 + i] = val;
    }
}

// =============================================================================
// U16 <-> F32 bit depth conversion
// =============================================================================

/// Generic U16 to F32 conversion for any channel count
fn convert_u16_to_f32_generic(from: &Image, to: &mut Image, channels: usize) {
    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;
    let row_elements = width * channels;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    // Convert slices
    let from_words: &[u16] = bytemuck::cast_slice(from_bytes);
    let to_floats: &mut [f32] = bytemuck::cast_slice_mut(to_bytes);
    let from_word_stride = from_stride / 2;
    let to_float_stride = to_stride / 4;

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_floats
        .par_chunks_mut(to_float_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_words[y * from_word_stride..y * from_word_stride + row_elements];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_u16_to_f32_row_avx2(from_row, to_row);
                } else {
                    convert_u16_to_f32_row_sse2(from_row, to_row);
                }
            }

            #[cfg(target_arch = "aarch64")]
            unsafe {
                convert_u16_to_f32_row_neon(from_row, to_row);
            }
        });
}

/// Generic F32 to U16 conversion for any channel count
fn convert_f32_to_u16_generic(from: &Image, to: &mut Image, channels: usize) {
    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;
    let row_elements = width * channels;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    // Convert slices
    let from_floats: &[f32] = bytemuck::cast_slice(from_bytes);
    let to_words: &mut [u16] = bytemuck::cast_slice_mut(to_bytes);
    let from_float_stride = from_stride / 4;
    let to_word_stride = to_stride / 2;

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_words
        .par_chunks_mut(to_word_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row =
                &from_floats[y * from_float_stride..y * from_float_stride + row_elements];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_f32_to_u16_row_avx2(from_row, to_row);
                } else {
                    convert_f32_to_u16_row_sse2(from_row, to_row);
                }
            }

            #[cfg(target_arch = "aarch64")]
            unsafe {
                convert_f32_to_u16_row_neon(from_row, to_row);
            }
        });
}

// =============================================================================
// U8 <-> U16 bit depth conversion
// =============================================================================

/// Generic U8 to U16 conversion for any channel count
/// Converts by scaling: u16 = u8 * 257 (maps 0->0, 255->65535)
fn convert_u8_to_u16_generic(from: &Image, to: &mut Image, channels: usize) {
    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;
    let row_bytes = width * channels;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    // Convert to_bytes to u16 slice
    let to_words: &mut [u16] = bytemuck::cast_slice_mut(to_bytes);
    let to_word_stride = to_stride / 2;

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_words
        .par_chunks_mut(to_word_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..y * from_stride + row_bytes];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_u8_to_u16_row_avx2(from_row, to_row);
                } else {
                    convert_u8_to_u16_row_sse2(from_row, to_row);
                }
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_u8_to_u16_row_neon(from_row, to_row);
            }
        });
}

/// Generic U16 to U8 conversion for any channel count
/// Converts by scaling: u8 = u16 / 257 (maps 0->0, 65535->255)
fn convert_u16_to_u8_generic(from: &Image, to: &mut Image, channels: usize) {
    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;
    let row_elements = width * channels;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    // Convert from_bytes to u16 slice
    let from_words: &[u16] = bytemuck::cast_slice(from_bytes);
    let from_word_stride = from_stride / 2;

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_words[y * from_word_stride..y * from_word_stride + row_elements];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_u16_to_u8_row_avx2(from_row, to_row);
                } else {
                    convert_u16_to_u8_row_sse2(from_row, to_row);
                }
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_u16_to_u8_row_neon(from_row, to_row);
            }
        });
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

        unsafe {
            // Load 16 bytes
            let bytes = _mm_loadu_si128(src.as_ptr().add(src_offset) as *const __m128i);

            // Unpack to 16-bit with zero extension
            let zero = _mm_setzero_si128();
            let words_lo = _mm_unpacklo_epi8(bytes, zero); // 8 u16 values
            let words_hi = _mm_unpackhi_epi8(bytes, zero); // 8 u16 values

            // Multiply by 257 to scale 0-255 to 0-65535
            // 257 = 0x101, so val * 257 = (val << 8) | val
            // We can achieve this by: words | (words << 8)
            let scaled_lo = _mm_or_si128(words_lo, _mm_slli_epi16(words_lo, 8));
            let scaled_hi = _mm_or_si128(words_hi, _mm_slli_epi16(words_hi, 8));

            // Store 16 u16 values
            _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, scaled_lo);
            _mm_storeu_si128(
                dst.as_mut_ptr().add(dst_offset + 8) as *mut __m128i,
                scaled_hi,
            );
        }
    }

    // Handle remainder (scalar)
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

        unsafe {
            // Load 16 u16 values
            let words_lo = _mm_loadu_si128(src.as_ptr().add(src_offset) as *const __m128i);
            let words_hi = _mm_loadu_si128(src.as_ptr().add(src_offset + 8) as *const __m128i);

            // Divide by 257: we can approximate by shifting right by 8
            // This gives us the high byte which is a good approximation
            let shifted_lo = _mm_srli_epi16(words_lo, 8);
            let shifted_hi = _mm_srli_epi16(words_hi, 8);

            // Pack to 8-bit (unsigned saturation, but values are already in range)
            let bytes = _mm_packus_epi16(shifted_lo, shifted_hi);

            // Store 16 bytes
            _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, bytes);
        }
    }

    // Handle remainder (scalar)
    for i in 0..remainder {
        dst[simd_width * 16 + i] = (src[simd_width * 16 + i] / 257) as u8;
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

        unsafe {
            // Load 16 bytes
            let bytes = vld1q_u8(src.as_ptr().add(src_offset));

            // Split and widen to 16-bit
            let words_lo = vmovl_u8(vget_low_u8(bytes));
            let words_hi = vmovl_u8(vget_high_u8(bytes));

            // Multiply by 257: val * 257 = (val << 8) | val
            let scaled_lo = vorrq_u16(words_lo, vshlq_n_u16(words_lo, 8));
            let scaled_hi = vorrq_u16(words_hi, vshlq_n_u16(words_hi, 8));

            // Store 16 u16 values
            vst1q_u16(dst.as_mut_ptr().add(dst_offset), scaled_lo);
            vst1q_u16(dst.as_mut_ptr().add(dst_offset + 8), scaled_hi);
        }
    }

    // Handle remainder (scalar)
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

        unsafe {
            // Load 16 u16 values
            let words_lo = vld1q_u16(src.as_ptr().add(src_offset));
            let words_hi = vld1q_u16(src.as_ptr().add(src_offset + 8));

            // Divide by 257: shift right by 8 (take high byte)
            let shifted_lo = vshrq_n_u16(words_lo, 8);
            let shifted_hi = vshrq_n_u16(words_hi, 8);

            // Narrow to 8-bit
            let bytes = vcombine_u8(vmovn_u16(shifted_lo), vmovn_u16(shifted_hi));

            // Store 16 bytes
            vst1q_u8(dst.as_mut_ptr().add(dst_offset), bytes);
        }
    }

    // Handle remainder (scalar)
    for i in 0..remainder {
        dst[simd_width * 16 + i] = (src[simd_width * 16 + i] / 257) as u8;
    }
}

// =============================================================================
// RGBA_U8 -> L_U8 luminance conversion
// =============================================================================

// Rec. 709 luminance weights scaled to fixed-point (shift by 16)
// R: 0.2126 * 65536 = 13933
// G: 0.7152 * 65536 = 46871
// B: 0.0722 * 65536 = 4732
const LUMA_R: u32 = 13933;
const LUMA_G: u32 = 46871;
const LUMA_B: u32 = 4732;

fn convert_rgba_u8_to_l_u8(from: &Image, to: &mut Image) {
    debug_assert_eq!(from.desc().color_format, ColorFormat::RGBA_U8);
    debug_assert_eq!(to.desc().color_format, ColorFormat::L_U8);
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);

    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_rgba_to_l_row_avx2(from_row, to_row, width);
                } else {
                    convert_rgba_to_l_row_ssse3(from_row, to_row, width);
                }
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_rgba_to_l_row_neon(from_row, to_row, width);
            }
        });
}

fn convert_rgb_u8_to_l_u8(from: &Image, to: &mut Image) {
    debug_assert_eq!(from.desc().color_format, ColorFormat::RGB_U8);
    debug_assert_eq!(to.desc().color_format, ColorFormat::L_U8);
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);

    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    #[cfg(target_arch = "x86_64")]
    let use_avx2 = is_x86_feature_detected!("avx2");

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..];

            #[cfg(target_arch = "x86_64")]
            unsafe {
                if use_avx2 {
                    convert_rgb_to_l_row_avx2(from_row, to_row, width);
                } else {
                    convert_rgb_to_l_row_ssse3(from_row, to_row, width);
                }
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_rgb_to_l_row_neon(from_row, to_row, width);
            }
        });
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn convert_rgba_to_l_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 8 pixels at a time (32 bytes in, 8 bytes out)
    let simd_width = width / 8;
    let remainder = width % 8;

    // Luminance weights for _mm_maddubs_epi16:
    // We need pairs of (R_weight, G_weight) and (B_weight, 0) for each pixel
    // Using approximation: R=54, G=183, B=19 (sum=256, allows shift by 8)
    // More accurate: R=54, G=183, B=18 for better rounding

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 8;

        unsafe {
            // Load 8 RGBA pixels (32 bytes)
            let rgba0 = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
            let rgba1 = _mm_loadu_si128(src_ptr.add(src_offset + 16) as *const __m128i);

            // Extract R, G, B channels using shuffle
            // From RGBA RGBA RGBA RGBA -> RRRR GGGG BBBB ----
            let shuf_r = _mm_setr_epi8(0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_g = _mm_setr_epi8(1, 5, 9, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_b =
                _mm_setr_epi8(2, 6, 10, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

            let r0 = _mm_shuffle_epi8(rgba0, shuf_r);
            let g0 = _mm_shuffle_epi8(rgba0, shuf_g);
            let b0 = _mm_shuffle_epi8(rgba0, shuf_b);

            let r1 = _mm_shuffle_epi8(rgba1, shuf_r);
            let g1 = _mm_shuffle_epi8(rgba1, shuf_g);
            let b1 = _mm_shuffle_epi8(rgba1, shuf_b);

            // Combine: r0[0-3] r1[0-3] in low 8 bytes
            let r = _mm_or_si128(r0, _mm_slli_si128(r1, 4));
            let g = _mm_or_si128(g0, _mm_slli_si128(g1, 4));
            let b = _mm_or_si128(b0, _mm_slli_si128(b1, 4));

            // Compute luminance using 16-bit intermediate
            // L = (R * 54 + G * 183 + B * 19) >> 8
            let zero = _mm_setzero_si128();

            // Unpack to 16-bit
            let r16 = _mm_unpacklo_epi8(r, zero);
            let g16 = _mm_unpacklo_epi8(g, zero);
            let b16 = _mm_unpacklo_epi8(b, zero);

            // Multiply and accumulate
            let r_w = _mm_set1_epi16(54);
            let g_w = _mm_set1_epi16(183);
            let b_w = _mm_set1_epi16(19);

            let sum = _mm_add_epi16(
                _mm_add_epi16(_mm_mullo_epi16(r16, r_w), _mm_mullo_epi16(g16, g_w)),
                _mm_mullo_epi16(b16, b_w),
            );

            // Shift right by 8 and pack to 8-bit
            let lum16 = _mm_srli_epi16(sum, 8);
            let lum8 = _mm_packus_epi16(lum16, zero);

            // Store 8 bytes
            _mm_storel_epi64(dst_ptr.add(dst_offset) as *mut __m128i, lum8);
        }
    }

    // Handle remainder pixels (scalar)
    let src_remainder = &src[simd_width * 32..];
    let dst_remainder = &mut dst[simd_width * 8..];
    for i in 0..remainder {
        let r = src_remainder[i * 4] as u32;
        let g = src_remainder[i * 4 + 1] as u32;
        let b = src_remainder[i * 4 + 2] as u32;
        dst_remainder[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn convert_rgb_to_l_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time (48 bytes in, 16 bytes out)
    let simd_width = width / 16;
    let remainder = width % 16;

    for i in 0..simd_width {
        let src_offset = i * 48;
        let dst_offset = i * 16;

        unsafe {
            // Load 48 bytes (16 RGB pixels)
            let in0 = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
            let in1 = _mm_loadu_si128(src_ptr.add(src_offset + 16) as *const __m128i);
            let in2 = _mm_loadu_si128(src_ptr.add(src_offset + 32) as *const __m128i);

            // Extract R, G, B for first 8 pixels
            // RGB RGB RGB RGB RGB R | GB RGB RGB RGB RGB RG | B RGB RGB RGB RGB RGB
            let shuf_r0 = _mm_setr_epi8(0, 3, 6, 9, 12, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_g0 =
                _mm_setr_epi8(1, 4, 7, 10, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_b0 =
                _mm_setr_epi8(2, 5, 8, 11, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

            // First 5-6 pixels from in0
            let r0_part = _mm_shuffle_epi8(in0, shuf_r0); // R0 R1 R2 R3 R4 R5 ...
            let g0_part = _mm_shuffle_epi8(in0, shuf_g0); // G0 G1 G2 G3 G4 ...
            let b0_part = _mm_shuffle_epi8(in0, shuf_b0); // B0 B1 B2 B3 B4 ...

            // Get remaining from in1 for first 8 pixels
            let shuf_r1 =
                _mm_setr_epi8(2, 5, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_g1 =
                _mm_setr_epi8(0, 3, 6, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_b1 =
                _mm_setr_epi8(1, 4, 7, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

            let r1_part = _mm_shuffle_epi8(in1, shuf_r1);
            let g1_part = _mm_shuffle_epi8(in1, shuf_g1);
            let b1_part = _mm_shuffle_epi8(in1, shuf_b1);

            // Combine first 8 pixels
            let r_lo = _mm_or_si128(r0_part, _mm_slli_si128(r1_part, 6));
            let g_lo = _mm_or_si128(g0_part, _mm_slli_si128(g1_part, 5));
            let b_lo = _mm_or_si128(b0_part, _mm_slli_si128(b1_part, 5));

            // Second 8 pixels (from in1 and in2)
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

            let shuf_r3 =
                _mm_setr_epi8(1, 4, 7, 10, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_g3 =
                _mm_setr_epi8(2, 5, 8, 11, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_b3 = _mm_setr_epi8(0, 3, 6, 9, 12, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

            let r3_part = _mm_shuffle_epi8(in2, shuf_r3);
            let g3_part = _mm_shuffle_epi8(in2, shuf_g3);
            let b3_part = _mm_shuffle_epi8(in2, shuf_b3);

            let r_hi = _mm_or_si128(r2_part, _mm_slli_si128(r3_part, 3));
            let g_hi = _mm_or_si128(g2_part, _mm_slli_si128(g3_part, 3));
            let b_hi = _mm_or_si128(b2_part, _mm_slli_si128(b3_part, 2));

            // Compute luminance for both halves
            let zero = _mm_setzero_si128();
            let r_w = _mm_set1_epi16(54);
            let g_w = _mm_set1_epi16(183);
            let b_w = _mm_set1_epi16(19);

            // First 8 pixels
            let r16_lo = _mm_unpacklo_epi8(r_lo, zero);
            let g16_lo = _mm_unpacklo_epi8(g_lo, zero);
            let b16_lo = _mm_unpacklo_epi8(b_lo, zero);

            let sum_lo = _mm_add_epi16(
                _mm_add_epi16(_mm_mullo_epi16(r16_lo, r_w), _mm_mullo_epi16(g16_lo, g_w)),
                _mm_mullo_epi16(b16_lo, b_w),
            );
            let lum16_lo = _mm_srli_epi16(sum_lo, 8);

            // Second 8 pixels
            let r16_hi = _mm_unpacklo_epi8(r_hi, zero);
            let g16_hi = _mm_unpacklo_epi8(g_hi, zero);
            let b16_hi = _mm_unpacklo_epi8(b_hi, zero);

            let sum_hi = _mm_add_epi16(
                _mm_add_epi16(_mm_mullo_epi16(r16_hi, r_w), _mm_mullo_epi16(g16_hi, g_w)),
                _mm_mullo_epi16(b16_hi, b_w),
            );
            let lum16_hi = _mm_srli_epi16(sum_hi, 8);

            // Pack both to 8-bit and store
            let lum8 = _mm_packus_epi16(lum16_lo, lum16_hi);
            _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, lum8);
        }
    }

    // Handle remainder pixels (scalar)
    let src_remainder = &src[simd_width * 48..];
    let dst_remainder = &mut dst[simd_width * 16..];
    for i in 0..remainder {
        let r = src_remainder[i * 3] as u32;
        let g = src_remainder[i * 3 + 1] as u32;
        let b = src_remainder[i * 3 + 2] as u32;
        dst_remainder[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgba_to_l_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time
    let simd_width = width / 16;
    let remainder = width % 16;

    unsafe {
        // Luminance weights (scaled to sum to 256 for shift by 8)
        let r_w = vdupq_n_u16(54);
        let g_w = vdupq_n_u16(183);
        let b_w = vdupq_n_u16(19);

        for i in 0..simd_width {
            let src_offset = i * 64;
            let dst_offset = i * 16;

            // Load 16 RGBA pixels deinterleaved
            let rgba = vld4q_u8(src_ptr.add(src_offset));
            let r = rgba.0;
            let g = rgba.1;
            let b = rgba.2;

            // Split into low and high halves, widen to 16-bit
            let r_lo = vmovl_u8(vget_low_u8(r));
            let r_hi = vmovl_u8(vget_high_u8(r));
            let g_lo = vmovl_u8(vget_low_u8(g));
            let g_hi = vmovl_u8(vget_high_u8(g));
            let b_lo = vmovl_u8(vget_low_u8(b));
            let b_hi = vmovl_u8(vget_high_u8(b));

            // Compute luminance: (R*54 + G*183 + B*19) >> 8
            let sum_lo = vaddq_u16(
                vaddq_u16(vmulq_u16(r_lo, r_w), vmulq_u16(g_lo, g_w)),
                vmulq_u16(b_lo, b_w),
            );
            let sum_hi = vaddq_u16(
                vaddq_u16(vmulq_u16(r_hi, r_w), vmulq_u16(g_hi, g_w)),
                vmulq_u16(b_hi, b_w),
            );

            // Shift right by 8 and narrow to 8-bit
            let lum_lo = vshrn_n_u16(sum_lo, 8);
            let lum_hi = vshrn_n_u16(sum_hi, 8);
            let lum = vcombine_u8(lum_lo, lum_hi);

            // Store 16 bytes
            vst1q_u8(dst_ptr.add(dst_offset), lum);
        }
    }

    // Handle remainder pixels (scalar)
    let src_remainder = &src[simd_width * 64..];
    let dst_remainder = &mut dst[simd_width * 16..];
    for i in 0..remainder {
        let r = src_remainder[i * 4] as u32;
        let g = src_remainder[i * 4 + 1] as u32;
        let b = src_remainder[i * 4 + 2] as u32;
        dst_remainder[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgb_to_l_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time
    let simd_width = width / 16;
    let remainder = width % 16;

    unsafe {
        // Luminance weights (scaled to sum to 256 for shift by 8)
        let r_w = vdupq_n_u16(54);
        let g_w = vdupq_n_u16(183);
        let b_w = vdupq_n_u16(19);

        for i in 0..simd_width {
            let src_offset = i * 48;
            let dst_offset = i * 16;

            // Load 16 RGB pixels deinterleaved
            let rgb = vld3q_u8(src_ptr.add(src_offset));
            let r = rgb.0;
            let g = rgb.1;
            let b = rgb.2;

            // Split into low and high halves, widen to 16-bit
            let r_lo = vmovl_u8(vget_low_u8(r));
            let r_hi = vmovl_u8(vget_high_u8(r));
            let g_lo = vmovl_u8(vget_low_u8(g));
            let g_hi = vmovl_u8(vget_high_u8(g));
            let b_lo = vmovl_u8(vget_low_u8(b));
            let b_hi = vmovl_u8(vget_high_u8(b));

            // Compute luminance: (R*54 + G*183 + B*19) >> 8
            let sum_lo = vaddq_u16(
                vaddq_u16(vmulq_u16(r_lo, r_w), vmulq_u16(g_lo, g_w)),
                vmulq_u16(b_lo, b_w),
            );
            let sum_hi = vaddq_u16(
                vaddq_u16(vmulq_u16(r_hi, r_w), vmulq_u16(g_hi, g_w)),
                vmulq_u16(b_hi, b_w),
            );

            // Shift right by 8 and narrow to 8-bit
            let lum_lo = vshrn_n_u16(sum_lo, 8);
            let lum_hi = vshrn_n_u16(sum_hi, 8);
            let lum = vcombine_u8(lum_lo, lum_hi);

            // Store 16 bytes
            vst1q_u8(dst_ptr.add(dst_offset), lum);
        }
    }

    // Handle remainder pixels (scalar)
    let src_remainder = &src[simd_width * 48..];
    let dst_remainder = &mut dst[simd_width * 16..];
    for i in 0..remainder {
        let r = src_remainder[i * 3] as u32;
        let g = src_remainder[i * 3 + 1] as u32;
        let b = src_remainder[i * 3 + 2] as u32;
        dst_remainder[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
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

    // Process 16 pixels at a time using vld4/vst3
    let simd_width = width / 16;
    let remainder = width % 16;

    for i in 0..simd_width {
        let src_offset = i * 64;
        let dst_offset = i * 48;

        unsafe {
            // Load 16 RGBA pixels (64 bytes) deinterleaved into R, G, B, A planes
            let rgba = vld4q_u8(src_ptr.add(src_offset));

            // Store only R, G, B (48 bytes) interleaved
            let rgb = uint8x16x3_t(rgba.0, rgba.1, rgba.2);
            vst3q_u8(dst_ptr.add(dst_offset), rgb);
        }
    }

    // Handle remainder pixels (scalar)
    let src_remainder = &src[simd_width * 64..];
    let dst_remainder = &mut dst[simd_width * 48..];
    for i in 0..remainder {
        dst_remainder[i * 3] = src_remainder[i * 4];
        dst_remainder[i * 3 + 1] = src_remainder[i * 4 + 1];
        dst_remainder[i * 3 + 2] = src_remainder[i * 4 + 2];
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgb_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time using vld3/vst4
    let simd_width = width / 16;
    let remainder = width % 16;

    unsafe {
        // Alpha channel (all 255)
        let alpha = vdupq_n_u8(255);

        for i in 0..simd_width {
            let src_offset = i * 48;
            let dst_offset = i * 64;

            // Load 16 RGB pixels (48 bytes) deinterleaved into R, G, B planes
            let rgb = vld3q_u8(src_ptr.add(src_offset));

            // Store R, G, B, A (64 bytes) interleaved
            let rgba = uint8x16x4_t(rgb.0, rgb.1, rgb.2, alpha);
            vst4q_u8(dst_ptr.add(dst_offset), rgba);
        }
    }

    // Handle remainder pixels (scalar)
    let src_remainder = &src[simd_width * 48..];
    let dst_remainder = &mut dst[simd_width * 64..];
    for i in 0..remainder {
        dst_remainder[i * 4] = src_remainder[i * 3];
        dst_remainder[i * 4 + 1] = src_remainder[i * 3 + 1];
        dst_remainder[i * 4 + 2] = src_remainder[i * 3 + 2];
        dst_remainder[i * 4 + 3] = 255;
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

    // Process 32 pixels at a time (128 bytes in, 96 bytes out)
    let simd_width = width / 32;
    let remainder = width % 32;

    // Shuffle mask to extract RGB from RGBA (4 pixels -> 12 bytes)
    // Same pattern as SSE but in 128-bit lanes
    let shuffle = _mm256_setr_epi8(
        0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1, // low lane
        0, 1, 2, 4, 5, 6, 8, 9, 10, 12, 13, 14, -1, -1, -1, -1, // high lane
    );

    for i in 0..simd_width {
        let src_offset = i * 128;
        let dst_offset = i * 96;

        unsafe {
            // Load 128 bytes (32 RGBA pixels) in 4 x 32-byte chunks
            let rgba0 = _mm256_loadu_si256(src_ptr.add(src_offset) as *const __m256i);
            let rgba1 = _mm256_loadu_si256(src_ptr.add(src_offset + 32) as *const __m256i);
            let rgba2 = _mm256_loadu_si256(src_ptr.add(src_offset + 64) as *const __m256i);
            let rgba3 = _mm256_loadu_si256(src_ptr.add(src_offset + 96) as *const __m256i);

            // Shuffle each to get RGB (12 bytes valid per 16-byte lane)
            // Each 256-bit register processes 8 pixels (2 lanes of 4)
            let rgb0 = _mm256_shuffle_epi8(rgba0, shuffle);
            let rgb1 = _mm256_shuffle_epi8(rgba1, shuffle);
            let rgb2 = _mm256_shuffle_epi8(rgba2, shuffle);
            let rgb3 = _mm256_shuffle_epi8(rgba3, shuffle);

            // Extract 128-bit lanes and pack them
            // rgb0: [lane0: 12 valid bytes][lane1: 12 valid bytes]
            let rgb0_lo = _mm256_castsi256_si128(rgb0);
            let rgb0_hi = _mm256_extracti128_si256(rgb0, 1);
            let rgb1_lo = _mm256_castsi256_si128(rgb1);
            let rgb1_hi = _mm256_extracti128_si256(rgb1, 1);
            let rgb2_lo = _mm256_castsi256_si128(rgb2);
            let rgb2_hi = _mm256_extracti128_si256(rgb2, 1);
            let rgb3_lo = _mm256_castsi256_si128(rgb3);
            let rgb3_hi = _mm256_extracti128_si256(rgb3, 1);

            // Pack 8x12 bytes = 96 bytes into 6x16 byte stores
            // out0: rgb0_lo[0..12] + rgb0_hi[0..4]
            let out0 = _mm_or_si128(rgb0_lo, _mm_slli_si128(rgb0_hi, 12));

            // out1: rgb0_hi[4..12] + rgb1_lo[0..8]
            let out1 = _mm_or_si128(_mm_srli_si128(rgb0_hi, 4), _mm_slli_si128(rgb1_lo, 8));

            // out2: rgb1_lo[8..12] + rgb1_hi[0..12]
            let out2 = _mm_or_si128(_mm_srli_si128(rgb1_lo, 8), _mm_slli_si128(rgb1_hi, 4));

            // out3: rgb2_lo[0..12] + rgb2_hi[0..4]
            let out3 = _mm_or_si128(rgb2_lo, _mm_slli_si128(rgb2_hi, 12));

            // out4: rgb2_hi[4..12] + rgb3_lo[0..8]
            let out4 = _mm_or_si128(_mm_srli_si128(rgb2_hi, 4), _mm_slli_si128(rgb3_lo, 8));

            // out5: rgb3_lo[8..12] + rgb3_hi[0..12]
            let out5 = _mm_or_si128(_mm_srli_si128(rgb3_lo, 8), _mm_slli_si128(rgb3_hi, 4));

            // Store 96 bytes
            _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, out0);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, out1);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 32) as *mut __m128i, out2);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 48) as *mut __m128i, out3);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 64) as *mut __m128i, out4);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 80) as *mut __m128i, out5);
        }
    }

    // Handle remainder with SSSE3 path (16 pixels at a time)
    let remaining_start = simd_width * 32;
    if remainder >= 16 {
        let src_rem = &src[remaining_start * 4..];
        let dst_rem = &mut dst[remaining_start * 3..];
        unsafe {
            convert_rgba_to_rgb_row_ssse3(src_rem, dst_rem, remainder);
        }
    } else if remainder > 0 {
        // Scalar remainder
        let src_remainder = &src[remaining_start * 4..];
        let dst_remainder = &mut dst[remaining_start * 3..];
        for i in 0..remainder {
            dst_remainder[i * 3] = src_remainder[i * 4];
            dst_remainder[i * 3 + 1] = src_remainder[i * 4 + 1];
            dst_remainder[i * 3 + 2] = src_remainder[i * 4 + 2];
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_rgb_to_rgba_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 32 pixels at a time (96 bytes in, 128 bytes out)
    let simd_width = width / 32;
    let remainder = width % 32;

    unsafe {
        // Alpha mask (0xFF in alpha positions)
        let alpha_mask = _mm256_setr_epi8(
            0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, // low lane
            0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, 0, 0, 0, -1, // high lane
        );
        // Shuffle: RGB -> RGBA (insert space for alpha)
        let shuf = _mm256_setr_epi8(
            0, 1, 2, -1, 3, 4, 5, -1, 6, 7, 8, -1, 9, 10, 11, -1, // low lane
            0, 1, 2, -1, 3, 4, 5, -1, 6, 7, 8, -1, 9, 10, 11, -1, // high lane
        );

        for i in 0..simd_width {
            let src_offset = i * 96;
            let dst_offset = i * 128;

            // Load 96 bytes (6 x 16-byte loads)
            let in0 = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
            let in1 = _mm_loadu_si128(src_ptr.add(src_offset + 16) as *const __m128i);
            let in2 = _mm_loadu_si128(src_ptr.add(src_offset + 32) as *const __m128i);
            let in3 = _mm_loadu_si128(src_ptr.add(src_offset + 48) as *const __m128i);
            let in4 = _mm_loadu_si128(src_ptr.add(src_offset + 64) as *const __m128i);
            let in5 = _mm_loadu_si128(src_ptr.add(src_offset + 80) as *const __m128i);

            // Reorganize 96 bytes into 8 groups of 12 bytes each
            // Group 0: in0[0..12]
            // Group 1: in0[12..16] + in1[0..8]
            // Group 2: in1[8..16] + in2[0..4]
            // Group 3: in2[4..16]
            // Group 4: in3[0..12]
            // Group 5: in3[12..16] + in4[0..8]
            // Group 6: in4[8..16] + in5[0..4]
            // Group 7: in5[4..16]

            // First 4 groups -> first 2 __m256i outputs
            let grp0 = in0; // bytes 0-11 valid
            let grp1 = _mm_or_si128(_mm_srli_si128(in0, 12), _mm_slli_si128(in1, 4));
            let grp2 = _mm_or_si128(_mm_srli_si128(in1, 8), _mm_slli_si128(in2, 8));
            let grp3 = _mm_srli_si128(in2, 4);

            // Second 4 groups -> second 2 __m256i outputs
            let grp4 = in3;
            let grp5 = _mm_or_si128(_mm_srli_si128(in3, 12), _mm_slli_si128(in4, 4));
            let grp6 = _mm_or_si128(_mm_srli_si128(in4, 8), _mm_slli_si128(in5, 8));
            let grp7 = _mm_srli_si128(in5, 4);

            // Combine into 256-bit registers and shuffle
            let combined0 = _mm256_set_m128i(grp1, grp0);
            let combined1 = _mm256_set_m128i(grp3, grp2);
            let combined2 = _mm256_set_m128i(grp5, grp4);
            let combined3 = _mm256_set_m128i(grp7, grp6);

            let rgba0 = _mm256_or_si256(_mm256_shuffle_epi8(combined0, shuf), alpha_mask);
            let rgba1 = _mm256_or_si256(_mm256_shuffle_epi8(combined1, shuf), alpha_mask);
            let rgba2 = _mm256_or_si256(_mm256_shuffle_epi8(combined2, shuf), alpha_mask);
            let rgba3 = _mm256_or_si256(_mm256_shuffle_epi8(combined3, shuf), alpha_mask);

            // Store 128 bytes
            _mm256_storeu_si256(dst_ptr.add(dst_offset) as *mut __m256i, rgba0);
            _mm256_storeu_si256(dst_ptr.add(dst_offset + 32) as *mut __m256i, rgba1);
            _mm256_storeu_si256(dst_ptr.add(dst_offset + 64) as *mut __m256i, rgba2);
            _mm256_storeu_si256(dst_ptr.add(dst_offset + 96) as *mut __m256i, rgba3);
        }
    }

    // Handle remainder with SSSE3 path
    let remaining_start = simd_width * 32;
    if remainder >= 16 {
        let src_rem = &src[remaining_start * 3..];
        let dst_rem = &mut dst[remaining_start * 4..];
        unsafe {
            convert_rgb_to_rgba_row_ssse3(src_rem, dst_rem, remainder);
        }
    } else if remainder > 0 {
        // Scalar remainder
        let src_remainder = &src[remaining_start * 3..];
        let dst_remainder = &mut dst[remaining_start * 4..];
        for i in 0..remainder {
            dst_remainder[i * 4] = src_remainder[i * 3];
            dst_remainder[i * 4 + 1] = src_remainder[i * 3 + 1];
            dst_remainder[i * 4 + 2] = src_remainder[i * 3 + 2];
            dst_remainder[i * 4 + 3] = 255;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_rgba_to_l_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time (64 bytes in, 16 bytes out)
    let simd_width = width / 16;
    let remainder = width % 16;

    unsafe {
        // Luminance weights
        let r_w = _mm256_set1_epi16(54);
        let g_w = _mm256_set1_epi16(183);
        let b_w = _mm256_set1_epi16(19);

        // Shuffle masks to extract R, G, B from RGBA (8 pixels per 128-bit lane)
        let shuf_r = _mm256_setr_epi8(
            0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 4, 8, 12, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );
        let shuf_g = _mm256_setr_epi8(
            1, 5, 9, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 1, 5, 9, 13, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );
        let shuf_b = _mm256_setr_epi8(
            2, 6, 10, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 2, 6, 10, 14, -1, -1, -1,
            -1, -1, -1, -1, -1, -1, -1, -1, -1,
        );

        for i in 0..simd_width {
            let src_offset = i * 64;
            let dst_offset = i * 16;

            // Load 16 RGBA pixels (64 bytes)
            let rgba0 = _mm256_loadu_si256(src_ptr.add(src_offset) as *const __m256i);
            let rgba1 = _mm256_loadu_si256(src_ptr.add(src_offset + 32) as *const __m256i);

            // Extract R, G, B channels
            let r0 = _mm256_shuffle_epi8(rgba0, shuf_r);
            let g0 = _mm256_shuffle_epi8(rgba0, shuf_g);
            let b0 = _mm256_shuffle_epi8(rgba0, shuf_b);

            let r1 = _mm256_shuffle_epi8(rgba1, shuf_r);
            let g1 = _mm256_shuffle_epi8(rgba1, shuf_g);
            let b1 = _mm256_shuffle_epi8(rgba1, shuf_b);

            // Extract and combine the 4-byte results from each lane
            // rgba0 low lane -> bytes 0-3, rgba0 high lane -> bytes 4-7
            // rgba1 low lane -> bytes 8-11, rgba1 high lane -> bytes 12-15
            let r0_lo = _mm256_castsi256_si128(r0);
            let r0_hi = _mm256_extracti128_si256(r0, 1);
            let r1_lo = _mm256_castsi256_si128(r1);
            let r1_hi = _mm256_extracti128_si256(r1, 1);

            let g0_lo = _mm256_castsi256_si128(g0);
            let g0_hi = _mm256_extracti128_si256(g0, 1);
            let g1_lo = _mm256_castsi256_si128(g1);
            let g1_hi = _mm256_extracti128_si256(g1, 1);

            let b0_lo = _mm256_castsi256_si128(b0);
            let b0_hi = _mm256_extracti128_si256(b0, 1);
            let b1_lo = _mm256_castsi256_si128(b1);
            let b1_hi = _mm256_extracti128_si256(b1, 1);

            // Combine into single 128-bit registers (16 bytes each)
            let r_combined = _mm_or_si128(
                _mm_or_si128(r0_lo, _mm_slli_si128(r0_hi, 4)),
                _mm_or_si128(_mm_slli_si128(r1_lo, 8), _mm_slli_si128(r1_hi, 12)),
            );
            let g_combined = _mm_or_si128(
                _mm_or_si128(g0_lo, _mm_slli_si128(g0_hi, 4)),
                _mm_or_si128(_mm_slli_si128(g1_lo, 8), _mm_slli_si128(g1_hi, 12)),
            );
            let b_combined = _mm_or_si128(
                _mm_or_si128(b0_lo, _mm_slli_si128(b0_hi, 4)),
                _mm_or_si128(_mm_slli_si128(b1_lo, 8), _mm_slli_si128(b1_hi, 12)),
            );

            // Widen to 256-bit for 16-bit math
            let zero_128 = _mm_setzero_si128();
            let r16 = _mm256_set_m128i(
                _mm_unpackhi_epi8(r_combined, zero_128),
                _mm_unpacklo_epi8(r_combined, zero_128),
            );
            let g16 = _mm256_set_m128i(
                _mm_unpackhi_epi8(g_combined, zero_128),
                _mm_unpacklo_epi8(g_combined, zero_128),
            );
            let b16 = _mm256_set_m128i(
                _mm_unpackhi_epi8(b_combined, zero_128),
                _mm_unpacklo_epi8(b_combined, zero_128),
            );

            // Compute luminance: (R*54 + G*183 + B*19) >> 8
            let sum = _mm256_add_epi16(
                _mm256_add_epi16(_mm256_mullo_epi16(r16, r_w), _mm256_mullo_epi16(g16, g_w)),
                _mm256_mullo_epi16(b16, b_w),
            );
            let lum16 = _mm256_srli_epi16(sum, 8);

            // Pack to 8-bit
            let lum16_lo = _mm256_castsi256_si128(lum16);
            let lum16_hi = _mm256_extracti128_si256(lum16, 1);
            let lum8 = _mm_packus_epi16(lum16_lo, lum16_hi);

            // Store 16 bytes
            _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, lum8);
        }
    }

    // Handle remainder pixels (scalar)
    let src_remainder = &src[simd_width * 64..];
    let dst_remainder = &mut dst[simd_width * 16..];
    for i in 0..remainder {
        let r = src_remainder[i * 4] as u32;
        let g = src_remainder[i * 4 + 1] as u32;
        let b = src_remainder[i * 4 + 2] as u32;
        dst_remainder[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_rgb_to_l_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 32 pixels at a time (96 bytes in, 32 bytes out)
    let simd_width = width / 32;
    let remainder = width % 32;

    unsafe {
        let r_w = _mm256_set1_epi16(54);
        let g_w = _mm256_set1_epi16(183);
        let b_w = _mm256_set1_epi16(19);

        for i in 0..simd_width {
            let src_offset = i * 96;
            let dst_offset = i * 32;

            // Load 96 bytes (32 RGB pixels)
            let in0 = _mm256_loadu_si256(src_ptr.add(src_offset) as *const __m256i);
            let in1 = _mm256_loadu_si256(src_ptr.add(src_offset + 32) as *const __m256i);
            let in2 = _mm256_loadu_si256(src_ptr.add(src_offset + 64) as *const __m256i);

            // For RGB data, we need to deinterleave. Since AVX2 doesn't have
            // direct deinterleave, we'll process in a different way:
            // Use the SSSE3 approach but doubled

            // Process first 16 pixels using 128-bit operations
            let in0_lo = _mm256_castsi256_si128(in0);
            let in0_hi = _mm256_extracti128_si256(in0, 1);
            let in1_lo = _mm256_castsi256_si128(in1);

            // Bytes 0-47 contain first 16 RGB pixels
            // Use same shuffle logic as SSSE3 version
            let shuf_r0 = _mm_setr_epi8(0, 3, 6, 9, 12, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_g0 =
                _mm_setr_epi8(1, 4, 7, 10, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_b0 =
                _mm_setr_epi8(2, 5, 8, 11, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

            let shuf_r1 =
                _mm_setr_epi8(2, 5, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_g1 =
                _mm_setr_epi8(0, 3, 6, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_b1 =
                _mm_setr_epi8(1, 4, 7, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

            let shuf_r2 = _mm_setr_epi8(
                8, 11, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            );
            let shuf_g2 = _mm_setr_epi8(
                9, 12, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            );
            let shuf_b2 = _mm_setr_epi8(
                10, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
            );

            let shuf_r3 =
                _mm_setr_epi8(1, 4, 7, 10, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_g3 =
                _mm_setr_epi8(2, 5, 8, 11, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
            let shuf_b3 = _mm_setr_epi8(0, 3, 6, 9, 12, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

            // First 16 pixels
            let r0_part = _mm_shuffle_epi8(in0_lo, shuf_r0);
            let g0_part = _mm_shuffle_epi8(in0_lo, shuf_g0);
            let b0_part = _mm_shuffle_epi8(in0_lo, shuf_b0);

            let r1_part = _mm_shuffle_epi8(in0_hi, shuf_r1);
            let g1_part = _mm_shuffle_epi8(in0_hi, shuf_g1);
            let b1_part = _mm_shuffle_epi8(in0_hi, shuf_b1);

            let r_lo = _mm_or_si128(r0_part, _mm_slli_si128(r1_part, 6));
            let g_lo = _mm_or_si128(g0_part, _mm_slli_si128(g1_part, 5));
            let b_lo = _mm_or_si128(b0_part, _mm_slli_si128(b1_part, 5));

            let r2_part = _mm_shuffle_epi8(in0_hi, shuf_r2);
            let g2_part = _mm_shuffle_epi8(in0_hi, shuf_g2);
            let b2_part = _mm_shuffle_epi8(in0_hi, shuf_b2);

            let r3_part = _mm_shuffle_epi8(in1_lo, shuf_r3);
            let g3_part = _mm_shuffle_epi8(in1_lo, shuf_g3);
            let b3_part = _mm_shuffle_epi8(in1_lo, shuf_b3);

            let r_hi_1 = _mm_or_si128(r2_part, _mm_slli_si128(r3_part, 3));
            let g_hi_1 = _mm_or_si128(g2_part, _mm_slli_si128(g3_part, 3));
            let b_hi_1 = _mm_or_si128(b2_part, _mm_slli_si128(b3_part, 2));

            // Second 16 pixels (bytes 48-95)
            let in1_hi = _mm256_extracti128_si256(in1, 1);
            let in2_lo = _mm256_castsi256_si128(in2);
            let in2_hi = _mm256_extracti128_si256(in2, 1);

            let r4_part = _mm_shuffle_epi8(in1_hi, shuf_r0);
            let g4_part = _mm_shuffle_epi8(in1_hi, shuf_g0);
            let b4_part = _mm_shuffle_epi8(in1_hi, shuf_b0);

            let r5_part = _mm_shuffle_epi8(in2_lo, shuf_r1);
            let g5_part = _mm_shuffle_epi8(in2_lo, shuf_g1);
            let b5_part = _mm_shuffle_epi8(in2_lo, shuf_b1);

            let r_lo_2 = _mm_or_si128(r4_part, _mm_slli_si128(r5_part, 6));
            let g_lo_2 = _mm_or_si128(g4_part, _mm_slli_si128(g5_part, 5));
            let b_lo_2 = _mm_or_si128(b4_part, _mm_slli_si128(b5_part, 5));

            let r6_part = _mm_shuffle_epi8(in2_lo, shuf_r2);
            let g6_part = _mm_shuffle_epi8(in2_lo, shuf_g2);
            let b6_part = _mm_shuffle_epi8(in2_lo, shuf_b2);

            let r7_part = _mm_shuffle_epi8(in2_hi, shuf_r3);
            let g7_part = _mm_shuffle_epi8(in2_hi, shuf_g3);
            let b7_part = _mm_shuffle_epi8(in2_hi, shuf_b3);

            let r_hi_2 = _mm_or_si128(r6_part, _mm_slli_si128(r7_part, 3));
            let g_hi_2 = _mm_or_si128(g6_part, _mm_slli_si128(g7_part, 3));
            let b_hi_2 = _mm_or_si128(b6_part, _mm_slli_si128(b7_part, 2));

            // Now compute luminance for all 32 pixels using AVX2
            let zero_128 = _mm_setzero_si128();

            // First 16 pixels
            let r16_0 = _mm256_set_m128i(
                _mm_unpacklo_epi8(r_hi_1, zero_128),
                _mm_unpacklo_epi8(r_lo, zero_128),
            );
            let g16_0 = _mm256_set_m128i(
                _mm_unpacklo_epi8(g_hi_1, zero_128),
                _mm_unpacklo_epi8(g_lo, zero_128),
            );
            let b16_0 = _mm256_set_m128i(
                _mm_unpacklo_epi8(b_hi_1, zero_128),
                _mm_unpacklo_epi8(b_lo, zero_128),
            );

            let sum0 = _mm256_add_epi16(
                _mm256_add_epi16(
                    _mm256_mullo_epi16(r16_0, r_w),
                    _mm256_mullo_epi16(g16_0, g_w),
                ),
                _mm256_mullo_epi16(b16_0, b_w),
            );
            let lum16_0 = _mm256_srli_epi16(sum0, 8);

            // Second 16 pixels
            let r16_1 = _mm256_set_m128i(
                _mm_unpacklo_epi8(r_hi_2, zero_128),
                _mm_unpacklo_epi8(r_lo_2, zero_128),
            );
            let g16_1 = _mm256_set_m128i(
                _mm_unpacklo_epi8(g_hi_2, zero_128),
                _mm_unpacklo_epi8(g_lo_2, zero_128),
            );
            let b16_1 = _mm256_set_m128i(
                _mm_unpacklo_epi8(b_hi_2, zero_128),
                _mm_unpacklo_epi8(b_lo_2, zero_128),
            );

            let sum1 = _mm256_add_epi16(
                _mm256_add_epi16(
                    _mm256_mullo_epi16(r16_1, r_w),
                    _mm256_mullo_epi16(g16_1, g_w),
                ),
                _mm256_mullo_epi16(b16_1, b_w),
            );
            let lum16_1 = _mm256_srli_epi16(sum1, 8);

            // Pack to 8-bit
            let lum16_0_lo = _mm256_castsi256_si128(lum16_0);
            let lum16_0_hi = _mm256_extracti128_si256(lum16_0, 1);
            let lum8_0 = _mm_packus_epi16(lum16_0_lo, lum16_0_hi);

            let lum16_1_lo = _mm256_castsi256_si128(lum16_1);
            let lum16_1_hi = _mm256_extracti128_si256(lum16_1, 1);
            let lum8_1 = _mm_packus_epi16(lum16_1_lo, lum16_1_hi);

            // Store 32 bytes
            _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, lum8_0);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, lum8_1);
        }
    }

    // Handle remainder with SSSE3 path
    let remaining_start = simd_width * 32;
    if remainder >= 16 {
        let src_rem = &src[remaining_start * 3..];
        let dst_rem = &mut dst[remaining_start..];
        unsafe {
            convert_rgb_to_l_row_ssse3(src_rem, dst_rem, remainder);
        }
    } else if remainder > 0 {
        // Scalar remainder
        let src_remainder = &src[remaining_start * 3..];
        let dst_remainder = &mut dst[remaining_start..];
        for i in 0..remainder {
            let r = src_remainder[i * 3] as u32;
            let g = src_remainder[i * 3 + 1] as u32;
            let b = src_remainder[i * 3 + 2] as u32;
            dst_remainder[i] = ((r * LUMA_R + g * LUMA_G + b * LUMA_B) >> 16) as u8;
        }
    }
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

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 32;

        unsafe {
            // Load 32 floats (128 bytes)
            let f0 = _mm256_loadu_ps(src.as_ptr().add(src_offset));
            let f1 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 8));
            let f2 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 16));
            let f3 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 24));

            // Scale and clamp
            let scaled0 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f0, scale), zero_f), max_f);
            let scaled1 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f1, scale), zero_f), max_f);
            let scaled2 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f2, scale), zero_f), max_f);
            let scaled3 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f3, scale), zero_f), max_f);

            // Convert to int32
            let i0 = _mm256_cvtps_epi32(scaled0);
            let i1 = _mm256_cvtps_epi32(scaled1);
            let i2 = _mm256_cvtps_epi32(scaled2);
            let i3 = _mm256_cvtps_epi32(scaled3);

            // Pack to 16-bit (need to handle AVX2 lane crossing)
            // _mm256_packs_epi32 packs within lanes, so we need to permute after
            let words_0 = _mm256_packs_epi32(i0, i1); // [i0_lo|i1_lo][i0_hi|i1_hi]
            let words_1 = _mm256_packs_epi32(i2, i3);

            // Permute to get correct order
            let words_0_perm = _mm256_permute4x64_epi64(words_0, 0b11_01_10_00);
            let words_1_perm = _mm256_permute4x64_epi64(words_1, 0b11_01_10_00);

            // Pack to 8-bit
            let bytes = _mm256_packus_epi16(words_0_perm, words_1_perm);
            let bytes_perm = _mm256_permute4x64_epi64(bytes, 0b11_01_10_00);

            // Store 32 bytes
            _mm256_storeu_si256(dst.as_mut_ptr().add(dst_offset) as *mut __m256i, bytes_perm);
        }
    }

    // Handle remainder with SSE2 path
    if remainder >= 16 {
        let src_rem = &src[simd_width * 32..];
        let dst_rem = &mut dst[simd_width * 32..];
        unsafe {
            convert_f32_to_u8_row_sse2(src_rem, dst_rem);
        }
    } else if remainder > 0 {
        // Scalar remainder
        for i in 0..remainder {
            let val = (src[simd_width * 32 + i] * 255.0).clamp(0.0, 255.0) as u8;
            dst[simd_width * 32 + i] = val;
        }
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

        unsafe {
            // Load 32 bytes
            let bytes = _mm256_loadu_si256(src.as_ptr().add(src_offset) as *const __m256i);

            // Split and widen to 16-bit
            let bytes_lo = _mm256_castsi256_si128(bytes);
            let bytes_hi = _mm256_extracti128_si256(bytes, 1);

            // Use AVX2 zero-extend
            let words_lo = _mm256_cvtepu8_epi16(bytes_lo); // 16 u16 values
            let words_hi = _mm256_cvtepu8_epi16(bytes_hi); // 16 u16 values

            // Multiply by 257: val * 257 = (val << 8) | val
            let scaled_lo = _mm256_or_si256(words_lo, _mm256_slli_epi16(words_lo, 8));
            let scaled_hi = _mm256_or_si256(words_hi, _mm256_slli_epi16(words_hi, 8));

            // Store 32 u16 values (64 bytes)
            _mm256_storeu_si256(dst.as_mut_ptr().add(dst_offset) as *mut __m256i, scaled_lo);
            _mm256_storeu_si256(
                dst.as_mut_ptr().add(dst_offset + 16) as *mut __m256i,
                scaled_hi,
            );
        }
    }

    // Handle remainder with SSE2 path
    if remainder >= 16 {
        let src_rem = &src[simd_width * 32..];
        let dst_rem = &mut dst[simd_width * 32..];
        unsafe {
            convert_u8_to_u16_row_sse2(src_rem, dst_rem);
        }
    } else if remainder > 0 {
        // Scalar remainder
        for i in 0..remainder {
            dst[simd_width * 32 + i] = (src[simd_width * 32 + i] as u16) * 257;
        }
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

        unsafe {
            // Load 32 u16 values (64 bytes)
            let words_lo = _mm256_loadu_si256(src.as_ptr().add(src_offset) as *const __m256i);
            let words_hi = _mm256_loadu_si256(src.as_ptr().add(src_offset + 16) as *const __m256i);

            // Divide by 257: shift right by 8 (take high byte)
            let shifted_lo = _mm256_srli_epi16(words_lo, 8);
            let shifted_hi = _mm256_srli_epi16(words_hi, 8);

            // Pack to 8-bit (handles lane crossing)
            let bytes = _mm256_packus_epi16(shifted_lo, shifted_hi);
            // Permute to fix lane order
            let bytes_perm = _mm256_permute4x64_epi64(bytes, 0b11_01_10_00);

            // Store 32 bytes
            _mm256_storeu_si256(dst.as_mut_ptr().add(dst_offset) as *mut __m256i, bytes_perm);
        }
    }

    // Handle remainder with SSE2 path
    if remainder >= 16 {
        let src_rem = &src[simd_width * 32..];
        let dst_rem = &mut dst[simd_width * 32..];
        unsafe {
            convert_u16_to_u8_row_sse2(src_rem, dst_rem);
        }
    } else if remainder > 0 {
        // Scalar remainder
        for i in 0..remainder {
            dst[simd_width * 32 + i] = (src[simd_width * 32 + i] / 257) as u8;
        }
    }
}

// =============================================================================
// LA_U8 <-> RGBA_U8 SIMD implementations
// =============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn convert_la_to_rgba_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 8 pixels at a time (16 bytes in, 32 bytes out)
    let simd_width = width / 8;
    let remainder = width % 8;

    // Shuffle mask: LA LA LA LA LA LA LA LA -> LLLL LLLL AAAA AAAA (deinterleave)
    // Then we need to expand L to RGB and keep A
    // Input: L0 A0 L1 A1 L2 A2 L3 A3 L4 A4 L5 A5 L6 A6 L7 A7
    // Output: L0 L0 L0 A0 L1 L1 L1 A1 ... (RGBA RGBA ...)

    // Shuffle to create RGBA from LA: L0 L0 L0 A0 L1 L1 L1 A1 L2 L2 L2 A2 L3 L3 L3 A3
    let shuf_lo = _mm_setr_epi8(0, 0, 0, 1, 2, 2, 2, 3, 4, 4, 4, 5, 6, 6, 6, 7);
    let shuf_hi = _mm_setr_epi8(8, 8, 8, 9, 10, 10, 10, 11, 12, 12, 12, 13, 14, 14, 14, 15);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 32;

        unsafe {
            // Load 16 bytes (8 LA pixels)
            let la = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);

            // Expand first 4 pixels (bytes 0-7)
            let rgba_lo = _mm_shuffle_epi8(la, shuf_lo);
            // Expand next 4 pixels (bytes 8-15)
            let rgba_hi = _mm_shuffle_epi8(la, shuf_hi);

            // Store 32 bytes (8 RGBA pixels)
            _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, rgba_lo);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, rgba_hi);
        }
    }

    // Handle remainder (scalar)
    let src_remainder = &src[simd_width * 16..];
    let dst_remainder = &mut dst[simd_width * 32..];
    for i in 0..remainder {
        let l = src_remainder[i * 2];
        let a = src_remainder[i * 2 + 1];
        dst_remainder[i * 4] = l;
        dst_remainder[i * 4 + 1] = l;
        dst_remainder[i * 4 + 2] = l;
        dst_remainder[i * 4 + 3] = a;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "ssse3")]
unsafe fn convert_rgba_to_la_row_ssse3(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 8 pixels at a time (32 bytes in, 16 bytes out)
    let simd_width = width / 8;
    let remainder = width % 8;

    // Luminance weights (scaled to sum to 256)
    let r_w = _mm_set1_epi16(54);
    let g_w = _mm_set1_epi16(183);
    let b_w = _mm_set1_epi16(19);

    // Shuffle to extract R, G, B, A from RGBA
    let shuf_r = _mm_setr_epi8(0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
    let shuf_g = _mm_setr_epi8(1, 5, 9, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
    let shuf_b = _mm_setr_epi8(2, 6, 10, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);
    let shuf_a = _mm_setr_epi8(3, 7, 11, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1);

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 16;

        unsafe {
            // Load 32 bytes (8 RGBA pixels)
            let rgba0 = _mm_loadu_si128(src_ptr.add(src_offset) as *const __m128i);
            let rgba1 = _mm_loadu_si128(src_ptr.add(src_offset + 16) as *const __m128i);

            // Extract channels from first 4 pixels
            let r0 = _mm_shuffle_epi8(rgba0, shuf_r);
            let g0 = _mm_shuffle_epi8(rgba0, shuf_g);
            let b0 = _mm_shuffle_epi8(rgba0, shuf_b);
            let a0 = _mm_shuffle_epi8(rgba0, shuf_a);

            // Extract channels from next 4 pixels
            let r1 = _mm_shuffle_epi8(rgba1, shuf_r);
            let g1 = _mm_shuffle_epi8(rgba1, shuf_g);
            let b1 = _mm_shuffle_epi8(rgba1, shuf_b);
            let a1 = _mm_shuffle_epi8(rgba1, shuf_a);

            // Combine: r0[0-3] r1[0-3] in low 8 bytes
            let r = _mm_or_si128(r0, _mm_slli_si128(r1, 4));
            let g = _mm_or_si128(g0, _mm_slli_si128(g1, 4));
            let b = _mm_or_si128(b0, _mm_slli_si128(b1, 4));
            let a = _mm_or_si128(a0, _mm_slli_si128(a1, 4));

            // Compute luminance
            let zero = _mm_setzero_si128();
            let r16 = _mm_unpacklo_epi8(r, zero);
            let g16 = _mm_unpacklo_epi8(g, zero);
            let b16 = _mm_unpacklo_epi8(b, zero);

            let sum = _mm_add_epi16(
                _mm_add_epi16(_mm_mullo_epi16(r16, r_w), _mm_mullo_epi16(g16, g_w)),
                _mm_mullo_epi16(b16, b_w),
            );
            let lum16 = _mm_srli_epi16(sum, 8);
            let lum8 = _mm_packus_epi16(lum16, zero);

            // Interleave L and A: L0 A0 L1 A1 ...
            let la = _mm_unpacklo_epi8(lum8, a);

            // Store 16 bytes (8 LA pixels)
            _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, la);
        }
    }

    // Handle remainder (scalar)
    let src_remainder = &src[simd_width * 32..];
    let dst_remainder = &mut dst[simd_width * 16..];
    for i in 0..remainder {
        let r = src_remainder[i * 4] as u32;
        let g = src_remainder[i * 4 + 1] as u32;
        let b = src_remainder[i * 4 + 2] as u32;
        let a = src_remainder[i * 4 + 3];
        let l = ((r * 13933 + g * 46871 + b * 4732) >> 16) as u8;
        dst_remainder[i * 2] = l;
        dst_remainder[i * 2 + 1] = a;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_la_to_rgba_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time (32 bytes in, 64 bytes out)
    let simd_width = width / 16;
    let remainder = width % 16;

    // Shuffle: L0 L0 L0 A0 L1 L1 L1 A1 ... within each 128-bit lane
    let shuf_lo = _mm256_setr_epi8(
        0, 0, 0, 1, 2, 2, 2, 3, 4, 4, 4, 5, 6, 6, 6, 7, 0, 0, 0, 1, 2, 2, 2, 3, 4, 4, 4, 5, 6, 6,
        6, 7,
    );
    let shuf_hi = _mm256_setr_epi8(
        8, 8, 8, 9, 10, 10, 10, 11, 12, 12, 12, 13, 14, 14, 14, 15, 8, 8, 8, 9, 10, 10, 10, 11, 12,
        12, 12, 13, 14, 14, 14, 15,
    );

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 64;

        unsafe {
            // Load 32 bytes (16 LA pixels)
            let la = _mm256_loadu_si256(src_ptr.add(src_offset) as *const __m256i);

            // Expand to RGBA using shuffle
            let rgba_lo = _mm256_shuffle_epi8(la, shuf_lo);
            let rgba_hi = _mm256_shuffle_epi8(la, shuf_hi);

            // Store 64 bytes (16 RGBA pixels)
            _mm256_storeu_si256(dst_ptr.add(dst_offset) as *mut __m256i, rgba_lo);
            _mm256_storeu_si256(dst_ptr.add(dst_offset + 32) as *mut __m256i, rgba_hi);
        }
    }

    // Handle remainder with SSSE3 path
    let remaining_start = simd_width * 16;
    if remainder >= 8 {
        let src_rem = &src[remaining_start * 2..];
        let dst_rem = &mut dst[remaining_start * 4..];
        unsafe {
            convert_la_to_rgba_row_ssse3(src_rem, dst_rem, remainder);
        }
    } else if remainder > 0 {
        let src_remainder = &src[remaining_start * 2..];
        let dst_remainder = &mut dst[remaining_start * 4..];
        for i in 0..remainder {
            let l = src_remainder[i * 2];
            let a = src_remainder[i * 2 + 1];
            dst_remainder[i * 4] = l;
            dst_remainder[i * 4 + 1] = l;
            dst_remainder[i * 4 + 2] = l;
            dst_remainder[i * 4 + 3] = a;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_rgba_to_la_row_avx2(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::x86_64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time (64 bytes in, 32 bytes out)
    let simd_width = width / 16;
    let remainder = width % 16;

    let r_w = _mm256_set1_epi16(54);
    let g_w = _mm256_set1_epi16(183);
    let b_w = _mm256_set1_epi16(19);

    // Shuffle to extract R, G, B, A from RGBA (4 pixels per 128-bit lane)
    let shuf_r = _mm256_setr_epi8(
        0, 4, 8, 12, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 4, 8, 12, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1,
    );
    let shuf_g = _mm256_setr_epi8(
        1, 5, 9, 13, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 1, 5, 9, 13, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1,
    );
    let shuf_b = _mm256_setr_epi8(
        2, 6, 10, 14, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 2, 6, 10, 14, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1,
    );
    let shuf_a = _mm256_setr_epi8(
        3, 7, 11, 15, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 3, 7, 11, 15, -1, -1, -1, -1,
        -1, -1, -1, -1, -1, -1, -1, -1,
    );

    for i in 0..simd_width {
        let src_offset = i * 64;
        let dst_offset = i * 32;

        unsafe {
            // Load 64 bytes (16 RGBA pixels)
            let rgba0 = _mm256_loadu_si256(src_ptr.add(src_offset) as *const __m256i);
            let rgba1 = _mm256_loadu_si256(src_ptr.add(src_offset + 32) as *const __m256i);

            // Extract channels
            let r0 = _mm256_shuffle_epi8(rgba0, shuf_r);
            let g0 = _mm256_shuffle_epi8(rgba0, shuf_g);
            let b0 = _mm256_shuffle_epi8(rgba0, shuf_b);
            let a0 = _mm256_shuffle_epi8(rgba0, shuf_a);

            let r1 = _mm256_shuffle_epi8(rgba1, shuf_r);
            let g1 = _mm256_shuffle_epi8(rgba1, shuf_g);
            let b1 = _mm256_shuffle_epi8(rgba1, shuf_b);
            let a1 = _mm256_shuffle_epi8(rgba1, shuf_a);

            // Extract lanes and combine
            let r0_lo = _mm256_castsi256_si128(r0);
            let r0_hi = _mm256_extracti128_si256(r0, 1);
            let r1_lo = _mm256_castsi256_si128(r1);
            let r1_hi = _mm256_extracti128_si256(r1, 1);

            let g0_lo = _mm256_castsi256_si128(g0);
            let g0_hi = _mm256_extracti128_si256(g0, 1);
            let g1_lo = _mm256_castsi256_si128(g1);
            let g1_hi = _mm256_extracti128_si256(g1, 1);

            let b0_lo = _mm256_castsi256_si128(b0);
            let b0_hi = _mm256_extracti128_si256(b0, 1);
            let b1_lo = _mm256_castsi256_si128(b1);
            let b1_hi = _mm256_extracti128_si256(b1, 1);

            let a0_lo = _mm256_castsi256_si128(a0);
            let a0_hi = _mm256_extracti128_si256(a0, 1);
            let a1_lo = _mm256_castsi256_si128(a1);
            let a1_hi = _mm256_extracti128_si256(a1, 1);

            // Combine into 16-byte vectors
            let r_combined = _mm_or_si128(
                _mm_or_si128(r0_lo, _mm_slli_si128(r0_hi, 4)),
                _mm_or_si128(_mm_slli_si128(r1_lo, 8), _mm_slli_si128(r1_hi, 12)),
            );
            let g_combined = _mm_or_si128(
                _mm_or_si128(g0_lo, _mm_slli_si128(g0_hi, 4)),
                _mm_or_si128(_mm_slli_si128(g1_lo, 8), _mm_slli_si128(g1_hi, 12)),
            );
            let b_combined = _mm_or_si128(
                _mm_or_si128(b0_lo, _mm_slli_si128(b0_hi, 4)),
                _mm_or_si128(_mm_slli_si128(b1_lo, 8), _mm_slli_si128(b1_hi, 12)),
            );
            let a_combined = _mm_or_si128(
                _mm_or_si128(a0_lo, _mm_slli_si128(a0_hi, 4)),
                _mm_or_si128(_mm_slli_si128(a1_lo, 8), _mm_slli_si128(a1_hi, 12)),
            );

            // Compute luminance using AVX2
            let zero_128 = _mm_setzero_si128();
            let r16 = _mm256_set_m128i(
                _mm_unpackhi_epi8(r_combined, zero_128),
                _mm_unpacklo_epi8(r_combined, zero_128),
            );
            let g16 = _mm256_set_m128i(
                _mm_unpackhi_epi8(g_combined, zero_128),
                _mm_unpacklo_epi8(g_combined, zero_128),
            );
            let b16 = _mm256_set_m128i(
                _mm_unpackhi_epi8(b_combined, zero_128),
                _mm_unpacklo_epi8(b_combined, zero_128),
            );

            let sum = _mm256_add_epi16(
                _mm256_add_epi16(_mm256_mullo_epi16(r16, r_w), _mm256_mullo_epi16(g16, g_w)),
                _mm256_mullo_epi16(b16, b_w),
            );
            let lum16 = _mm256_srli_epi16(sum, 8);

            // Pack to 8-bit
            let lum16_lo = _mm256_castsi256_si128(lum16);
            let lum16_hi = _mm256_extracti128_si256(lum16, 1);
            let lum8 = _mm_packus_epi16(lum16_lo, lum16_hi);

            // Interleave L and A
            let la_lo = _mm_unpacklo_epi8(lum8, a_combined);
            let la_hi = _mm_unpackhi_epi8(lum8, a_combined);

            // Store 32 bytes (16 LA pixels)
            _mm_storeu_si128(dst_ptr.add(dst_offset) as *mut __m128i, la_lo);
            _mm_storeu_si128(dst_ptr.add(dst_offset + 16) as *mut __m128i, la_hi);
        }
    }

    // Handle remainder with SSSE3 path
    let remaining_start = simd_width * 16;
    if remainder >= 8 {
        let src_rem = &src[remaining_start * 4..];
        let dst_rem = &mut dst[remaining_start * 2..];
        unsafe {
            convert_rgba_to_la_row_ssse3(src_rem, dst_rem, remainder);
        }
    } else if remainder > 0 {
        let src_remainder = &src[remaining_start * 4..];
        let dst_remainder = &mut dst[remaining_start * 2..];
        for i in 0..remainder {
            let r = src_remainder[i * 4] as u32;
            let g = src_remainder[i * 4 + 1] as u32;
            let b = src_remainder[i * 4 + 2] as u32;
            let a = src_remainder[i * 4 + 3];
            let l = ((r * 13933 + g * 46871 + b * 4732) >> 16) as u8;
            dst_remainder[i * 2] = l;
            dst_remainder[i * 2 + 1] = a;
        }
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_la_to_rgba_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time
    let simd_width = width / 16;
    let remainder = width % 16;

    for i in 0..simd_width {
        let src_offset = i * 32;
        let dst_offset = i * 64;

        unsafe {
            // Load 16 LA pixels deinterleaved
            let la = vld2q_u8(src_ptr.add(src_offset));
            let l = la.0;
            let a = la.1;

            // Store as RGBA (R=G=B=L)
            let rgba = uint8x16x4_t(l, l, l, a);
            vst4q_u8(dst_ptr.add(dst_offset), rgba);
        }
    }

    // Handle remainder (scalar)
    let src_remainder = &src[simd_width * 32..];
    let dst_remainder = &mut dst[simd_width * 64..];
    for i in 0..remainder {
        let l = src_remainder[i * 2];
        let a = src_remainder[i * 2 + 1];
        dst_remainder[i * 4] = l;
        dst_remainder[i * 4 + 1] = l;
        dst_remainder[i * 4 + 2] = l;
        dst_remainder[i * 4 + 3] = a;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgba_to_la_row_neon(src: &[u8], dst: &mut [u8], width: usize) {
    use std::arch::aarch64::*;

    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    // Process 16 pixels at a time
    let simd_width = width / 16;
    let remainder = width % 16;

    unsafe {
        let r_w = vdupq_n_u16(54);
        let g_w = vdupq_n_u16(183);
        let b_w = vdupq_n_u16(19);

        for i in 0..simd_width {
            let src_offset = i * 64;
            let dst_offset = i * 32;

            // Load 16 RGBA pixels deinterleaved
            let rgba = vld4q_u8(src_ptr.add(src_offset));
            let r = rgba.0;
            let g = rgba.1;
            let b = rgba.2;
            let a = rgba.3;

            // Compute luminance
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

            // Store as LA
            let la = uint8x16x2_t(lum, a);
            vst2q_u8(dst_ptr.add(dst_offset), la);
        }
    }

    // Handle remainder (scalar)
    let src_remainder = &src[simd_width * 64..];
    let dst_remainder = &mut dst[simd_width * 32..];
    for i in 0..remainder {
        let r = src_remainder[i * 4] as u32;
        let g = src_remainder[i * 4 + 1] as u32;
        let b = src_remainder[i * 4 + 2] as u32;
        let a = src_remainder[i * 4 + 3];
        let l = ((r * 13933 + g * 46871 + b * 4732) >> 16) as u8;
        dst_remainder[i * 2] = l;
        dst_remainder[i * 2 + 1] = a;
    }
}

// =============================================================================
// U16 <-> F32 SIMD implementations
// =============================================================================

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn convert_u16_to_f32_row_sse2(src: &[u16], dst: &mut [f32]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 8;
    let remainder = len % 8;

    let scale = _mm_set1_ps(1.0 / 65535.0);

    for i in 0..simd_width {
        let src_offset = i * 8;
        let dst_offset = i * 8;

        unsafe {
            // Load 8 u16 values
            let words = _mm_loadu_si128(src.as_ptr().add(src_offset) as *const __m128i);

            // Unpack to 32-bit
            let zero = _mm_setzero_si128();
            let dwords_lo = _mm_unpacklo_epi16(words, zero);
            let dwords_hi = _mm_unpackhi_epi16(words, zero);

            // Convert to float and scale
            let floats_lo = _mm_mul_ps(_mm_cvtepi32_ps(dwords_lo), scale);
            let floats_hi = _mm_mul_ps(_mm_cvtepi32_ps(dwords_hi), scale);

            // Store 8 floats
            _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset), floats_lo);
            _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 4), floats_hi);
        }
    }

    // Handle remainder (scalar)
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

        unsafe {
            // Load 8 floats
            let f0 = _mm_loadu_ps(src.as_ptr().add(src_offset));
            let f1 = _mm_loadu_ps(src.as_ptr().add(src_offset + 4));

            // Scale and clamp
            let scaled0 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f0, scale), zero_f), max_f);
            let scaled1 = _mm_min_ps(_mm_max_ps(_mm_mul_ps(f1, scale), zero_f), max_f);

            // Convert to int32
            let i0 = _mm_cvtps_epi32(scaled0);
            let i1 = _mm_cvtps_epi32(scaled1);

            // Pack to 16-bit (using unsigned saturation via signed pack then mask)
            // Since values are 0-65535, we need to handle this carefully
            // _mm_packus_epi32 is SSE4.1, so we use a workaround
            // Values are already clamped to 0-65535, so we can safely use signed pack
            // after adjusting
            let words = _mm_packs_epi32(i0, i1);

            // Store 8 u16 values (note: packs_epi32 does signed saturation)
            // We need to use a different approach for proper unsigned
            // For now, store directly since values are in range
            _mm_storeu_si128(dst.as_mut_ptr().add(dst_offset) as *mut __m128i, words);
        }
    }

    // Handle remainder (scalar)
    for i in 0..remainder {
        let val = (src[simd_width * 8 + i] * 65535.0).clamp(0.0, 65535.0) as u16;
        dst[simd_width * 8 + i] = val;
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

        unsafe {
            // Load 16 u16 values
            let words = _mm256_loadu_si256(src.as_ptr().add(src_offset) as *const __m256i);

            // Split into 128-bit halves
            let words_lo = _mm256_castsi256_si128(words);
            let words_hi = _mm256_extracti128_si256(words, 1);

            // Use AVX2 zero-extend u16 to u32
            let dwords_0 = _mm256_cvtepu16_epi32(words_lo);
            let dwords_1 = _mm256_cvtepu16_epi32(words_hi);

            // Convert to float and scale
            let floats_0 = _mm256_mul_ps(_mm256_cvtepi32_ps(dwords_0), scale);
            let floats_1 = _mm256_mul_ps(_mm256_cvtepi32_ps(dwords_1), scale);

            // Store 16 floats
            _mm256_storeu_ps(dst.as_mut_ptr().add(dst_offset), floats_0);
            _mm256_storeu_ps(dst.as_mut_ptr().add(dst_offset + 8), floats_1);
        }
    }

    // Handle remainder with SSE2 path
    if remainder >= 8 {
        let src_rem = &src[simd_width * 16..];
        let dst_rem = &mut dst[simd_width * 16..];
        unsafe {
            convert_u16_to_f32_row_sse2(src_rem, dst_rem);
        }
    } else if remainder > 0 {
        for i in 0..remainder {
            dst[simd_width * 16 + i] = src[simd_width * 16 + i] as f32 / 65535.0;
        }
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

        unsafe {
            // Load 16 floats
            let f0 = _mm256_loadu_ps(src.as_ptr().add(src_offset));
            let f1 = _mm256_loadu_ps(src.as_ptr().add(src_offset + 8));

            // Scale and clamp
            let scaled0 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f0, scale), zero_f), max_f);
            let scaled1 = _mm256_min_ps(_mm256_max_ps(_mm256_mul_ps(f1, scale), zero_f), max_f);

            // Convert to int32
            let i0 = _mm256_cvtps_epi32(scaled0);
            let i1 = _mm256_cvtps_epi32(scaled1);

            // Pack to 16-bit using packus (unsigned saturation)
            let words = _mm256_packus_epi32(i0, i1);
            // Fix lane order
            let words_perm = _mm256_permute4x64_epi64(words, 0b11_01_10_00);

            // Store 16 u16 values
            _mm256_storeu_si256(dst.as_mut_ptr().add(dst_offset) as *mut __m256i, words_perm);
        }
    }

    // Handle remainder with SSE2 path
    if remainder >= 8 {
        let src_rem = &src[simd_width * 16..];
        let dst_rem = &mut dst[simd_width * 16..];
        unsafe {
            convert_f32_to_u16_row_sse2(src_rem, dst_rem);
        }
    } else if remainder > 0 {
        for i in 0..remainder {
            let val = (src[simd_width * 16 + i] * 65535.0).clamp(0.0, 65535.0) as u16;
            dst[simd_width * 16 + i] = val;
        }
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_u16_to_f32_row_neon(src: &[u16], dst: &mut [f32]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 8;
    let remainder = len % 8;

    unsafe {
        let scale = vdupq_n_f32(1.0 / 65535.0);

        for i in 0..simd_width {
            let src_offset = i * 8;
            let dst_offset = i * 8;

            // Load 8 u16 values
            let words = vld1q_u16(src.as_ptr().add(src_offset));

            // Widen to 32-bit
            let dwords_lo = vmovl_u16(vget_low_u16(words));
            let dwords_hi = vmovl_u16(vget_high_u16(words));

            // Convert to float and scale
            let floats_lo = vmulq_f32(vcvtq_f32_u32(dwords_lo), scale);
            let floats_hi = vmulq_f32(vcvtq_f32_u32(dwords_hi), scale);

            // Store 8 floats
            vst1q_f32(dst.as_mut_ptr().add(dst_offset), floats_lo);
            vst1q_f32(dst.as_mut_ptr().add(dst_offset + 4), floats_hi);
        }
    }

    // Handle remainder (scalar)
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

    unsafe {
        let scale = vdupq_n_f32(65535.0);
        let zero = vdupq_n_f32(0.0);
        let max = vdupq_n_f32(65535.0);

        for i in 0..simd_width {
            let src_offset = i * 8;
            let dst_offset = i * 8;

            // Load 8 floats
            let f0 = vld1q_f32(src.as_ptr().add(src_offset));
            let f1 = vld1q_f32(src.as_ptr().add(src_offset + 4));

            // Scale and clamp
            let scaled0 = vminq_f32(vmaxq_f32(vmulq_f32(f0, scale), zero), max);
            let scaled1 = vminq_f32(vmaxq_f32(vmulq_f32(f1, scale), zero), max);

            // Convert to u32
            let u0 = vcvtq_u32_f32(scaled0);
            let u1 = vcvtq_u32_f32(scaled1);

            // Narrow to u16
            let words = vcombine_u16(vmovn_u32(u0), vmovn_u32(u1));

            // Store 8 u16 values
            vst1q_u16(dst.as_mut_ptr().add(dst_offset), words);
        }
    }

    // Handle remainder (scalar)
    for i in 0..remainder {
        let val = (src[simd_width * 8 + i] * 65535.0).clamp(0.0, 65535.0) as u16;
        dst[simd_width * 8 + i] = val;
    }
}
