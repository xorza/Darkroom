// SIMD-optimized image conversion implementations
//
// This module contains SIMD implementations for common conversion paths:
// - RGBA_U8 <-> RGB_U8
// - RGB_U8/RGBA_U8 -> L_U8 (luminance)
// - U8 <-> F32 (bit depth conversion)
// - More to be added...

use rayon::prelude::*;

use crate::common::color_format::ColorFormat;
use crate::common::error::Result;
use crate::image::Image;

/// Check if SIMD conversion is available for the given format pair.
/// Returns true if a SIMD-optimized path exists.
pub(crate) fn has_simd_path(from: &Image, to: &Image) -> bool {
    let from_fmt = from.desc().color_format;
    let to_fmt = to.desc().color_format;

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("sse2") {
        match (from_fmt, to_fmt) {
            // Channel conversions (SSSE3)
            (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => return true,
            (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) => return true,
            // Luminance (SSSE3)
            (ColorFormat::RGBA_U8, ColorFormat::L_U8) => return true,
            (ColorFormat::RGB_U8, ColorFormat::L_U8) => return true,
            // U8<->F32 (SSE2)
            (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32) => return true,
            (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => return true,
            (ColorFormat::RGB_U8, ColorFormat::RGB_F32) => return true,
            (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => return true,
            (ColorFormat::L_U8, ColorFormat::L_F32) => return true,
            (ColorFormat::L_F32, ColorFormat::L_U8) => return true,
            (ColorFormat::LA_U8, ColorFormat::LA_F32) => return true,
            (ColorFormat::LA_F32, ColorFormat::LA_U8) => return true,
            // U8<->U16 (SSE2)
            (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16) => return true,
            (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8) => return true,
            (ColorFormat::RGB_U8, ColorFormat::RGB_U16) => return true,
            (ColorFormat::RGB_U16, ColorFormat::RGB_U8) => return true,
            (ColorFormat::L_U8, ColorFormat::L_U16) => return true,
            (ColorFormat::L_U16, ColorFormat::L_U8) => return true,
            (ColorFormat::LA_U8, ColorFormat::LA_U16) => return true,
            (ColorFormat::LA_U16, ColorFormat::LA_U8) => return true,
            // F32 channel conversions (SSE)
            (ColorFormat::RGBA_F32, ColorFormat::RGB_F32) => return true,
            (ColorFormat::RGB_F32, ColorFormat::RGBA_F32) => return true,
            _ => {}
        }
    }

    #[cfg(target_arch = "aarch64")]
    match (from_fmt, to_fmt) {
        // Channel conversions
        (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => return true,
        (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) => return true,
        // F32 channel conversions
        (ColorFormat::RGBA_F32, ColorFormat::RGB_F32) => return true,
        (ColorFormat::RGB_F32, ColorFormat::RGBA_F32) => return true,
        // Luminance
        (ColorFormat::RGBA_U8, ColorFormat::L_U8) => return true,
        (ColorFormat::RGB_U8, ColorFormat::L_U8) => return true,
        // U8<->F32
        (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32) => return true,
        (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => return true,
        (ColorFormat::RGB_U8, ColorFormat::RGB_F32) => return true,
        (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => return true,
        (ColorFormat::L_U8, ColorFormat::L_F32) => return true,
        (ColorFormat::L_F32, ColorFormat::L_U8) => return true,
        (ColorFormat::LA_U8, ColorFormat::LA_F32) => return true,
        (ColorFormat::LA_F32, ColorFormat::LA_U8) => return true,
        // U8<->U16
        (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16) => return true,
        (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8) => return true,
        (ColorFormat::RGB_U8, ColorFormat::RGB_U16) => return true,
        (ColorFormat::RGB_U16, ColorFormat::RGB_U8) => return true,
        (ColorFormat::L_U8, ColorFormat::L_U16) => return true,
        (ColorFormat::L_U16, ColorFormat::L_U8) => return true,
        (ColorFormat::LA_U8, ColorFormat::LA_U16) => return true,
        (ColorFormat::LA_U16, ColorFormat::LA_U8) => return true,
        _ => {}
    }

    false
}

/// Attempt SIMD conversion. Returns Ok(true) if conversion was performed,
/// Ok(false) if no SIMD path available, or Err on failure.
pub(crate) fn try_convert_simd(from: &Image, to: &mut Image) -> Result<bool> {
    let from_fmt = from.desc().color_format;
    let to_fmt = to.desc().color_format;

    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("sse2") {
        match (from_fmt, to_fmt) {
            // Channel conversions (require SSSE3)
            (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) if is_x86_feature_detected!("ssse3") => {
                convert_rgba_u8_to_rgb_u8(from, to);
                return Ok(true);
            }
            (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) if is_x86_feature_detected!("ssse3") => {
                convert_rgb_u8_to_rgba_u8(from, to);
                return Ok(true);
            }
            // Luminance (require SSSE3)
            (ColorFormat::RGBA_U8, ColorFormat::L_U8) if is_x86_feature_detected!("ssse3") => {
                convert_rgba_u8_to_l_u8(from, to);
                return Ok(true);
            }
            (ColorFormat::RGB_U8, ColorFormat::L_U8) if is_x86_feature_detected!("ssse3") => {
                convert_rgb_u8_to_l_u8(from, to);
                return Ok(true);
            }
            // U8<->F32 (SSE2 is sufficient)
            (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32) => {
                convert_u8_to_f32_generic(from, to, 4);
                return Ok(true);
            }
            (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => {
                convert_f32_to_u8_generic(from, to, 4);
                return Ok(true);
            }
            (ColorFormat::RGB_U8, ColorFormat::RGB_F32) => {
                convert_u8_to_f32_generic(from, to, 3);
                return Ok(true);
            }
            (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => {
                convert_f32_to_u8_generic(from, to, 3);
                return Ok(true);
            }
            (ColorFormat::L_U8, ColorFormat::L_F32) => {
                convert_u8_to_f32_generic(from, to, 1);
                return Ok(true);
            }
            (ColorFormat::L_F32, ColorFormat::L_U8) => {
                convert_f32_to_u8_generic(from, to, 1);
                return Ok(true);
            }
            (ColorFormat::LA_U8, ColorFormat::LA_F32) => {
                convert_u8_to_f32_generic(from, to, 2);
                return Ok(true);
            }
            (ColorFormat::LA_F32, ColorFormat::LA_U8) => {
                convert_f32_to_u8_generic(from, to, 2);
                return Ok(true);
            }
            // U8<->U16 (SSE2 is sufficient)
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
            // F32 channel conversions
            (ColorFormat::RGBA_F32, ColorFormat::RGB_F32) => {
                convert_rgba_f32_to_rgb_f32(from, to);
                return Ok(true);
            }
            (ColorFormat::RGB_F32, ColorFormat::RGBA_F32) => {
                convert_rgb_f32_to_rgba_f32(from, to);
                return Ok(true);
            }
            _ => {}
        }
    }

    #[cfg(target_arch = "aarch64")]
    match (from_fmt, to_fmt) {
        // Channel conversions
        (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => {
            convert_rgba_u8_to_rgb_u8(from, to);
            return Ok(true);
        }
        (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) => {
            convert_rgb_u8_to_rgba_u8(from, to);
            return Ok(true);
        }
        // F32 channel conversions
        (ColorFormat::RGBA_F32, ColorFormat::RGB_F32) => {
            convert_rgba_f32_to_rgb_f32(from, to);
            return Ok(true);
        }
        (ColorFormat::RGB_F32, ColorFormat::RGBA_F32) => {
            convert_rgb_f32_to_rgba_f32(from, to);
            return Ok(true);
        }
        // Luminance
        (ColorFormat::RGBA_U8, ColorFormat::L_U8) => {
            convert_rgba_u8_to_l_u8(from, to);
            return Ok(true);
        }
        (ColorFormat::RGB_U8, ColorFormat::L_U8) => {
            convert_rgb_u8_to_l_u8(from, to);
            return Ok(true);
        }
        // U8<->F32
        (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32) => {
            convert_u8_to_f32_generic(from, to, 4);
            return Ok(true);
        }
        (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => {
            convert_f32_to_u8_generic(from, to, 4);
            return Ok(true);
        }
        (ColorFormat::RGB_U8, ColorFormat::RGB_F32) => {
            convert_u8_to_f32_generic(from, to, 3);
            return Ok(true);
        }
        (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => {
            convert_f32_to_u8_generic(from, to, 3);
            return Ok(true);
        }
        (ColorFormat::L_U8, ColorFormat::L_F32) => {
            convert_u8_to_f32_generic(from, to, 1);
            return Ok(true);
        }
        (ColorFormat::L_F32, ColorFormat::L_U8) => {
            convert_f32_to_u8_generic(from, to, 1);
            return Ok(true);
        }
        (ColorFormat::LA_U8, ColorFormat::LA_F32) => {
            convert_u8_to_f32_generic(from, to, 2);
            return Ok(true);
        }
        (ColorFormat::LA_F32, ColorFormat::LA_U8) => {
            convert_f32_to_u8_generic(from, to, 2);
            return Ok(true);
        }
        // U8<->U16
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

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..];

            #[cfg(target_arch = "x86_64")]
            // SAFETY: We've verified SSSE3 is available in try_convert_simd
            unsafe {
                convert_rgba_to_rgb_row_ssse3(from_row, to_row, width);
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

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..];

            #[cfg(target_arch = "x86_64")]
            // SAFETY: We've verified SSSE3 is available in try_convert_simd
            unsafe {
                convert_rgb_to_rgba_row_ssse3(from_row, to_row, width);
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_rgb_to_rgba_row_neon(from_row, to_row, width);
            }
        });
}

// =============================================================================
// RGBA_F32 <-> RGB_F32 conversion
// =============================================================================

fn convert_rgba_f32_to_rgb_f32(from: &Image, to: &mut Image) {
    debug_assert_eq!(from.desc().color_format, ColorFormat::RGBA_F32);
    debug_assert_eq!(to.desc().color_format, ColorFormat::RGB_F32);
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);

    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    // Convert to f32 slices
    let from_floats: &[f32] = bytemuck::cast_slice(from_bytes);
    let to_floats: &mut [f32] = bytemuck::cast_slice_mut(to_bytes);
    let from_float_stride = from_stride / 4;
    let to_float_stride = to_stride / 4;

    to_floats
        .par_chunks_mut(to_float_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_floats[y * from_float_stride..];

            #[cfg(target_arch = "x86_64")]
            // SAFETY: SSE is always available on x86_64
            unsafe {
                convert_rgba_f32_to_rgb_f32_row_sse(from_row, to_row, width);
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_rgba_f32_to_rgb_f32_row_neon(from_row, to_row, width);
            }
        });
}

fn convert_rgb_f32_to_rgba_f32(from: &Image, to: &mut Image) {
    debug_assert_eq!(from.desc().color_format, ColorFormat::RGB_F32);
    debug_assert_eq!(to.desc().color_format, ColorFormat::RGBA_F32);
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);

    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    // Convert to f32 slices
    let from_floats: &[f32] = bytemuck::cast_slice(from_bytes);
    let to_floats: &mut [f32] = bytemuck::cast_slice_mut(to_bytes);
    let from_float_stride = from_stride / 4;
    let to_float_stride = to_stride / 4;

    to_floats
        .par_chunks_mut(to_float_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_floats[y * from_float_stride..];

            #[cfg(target_arch = "x86_64")]
            // SAFETY: SSE is always available on x86_64
            unsafe {
                convert_rgb_f32_to_rgba_f32_row_sse(from_row, to_row, width);
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_rgb_f32_to_rgba_f32_row_neon(from_row, to_row, width);
            }
        });
}

#[cfg(target_arch = "x86_64")]
unsafe fn convert_rgba_f32_to_rgb_f32_row_sse(src: &[f32], dst: &mut [f32], width: usize) {
    // F32 channel conversion uses scalar loop with rayon parallelization.
    // The 3<->4 channel shuffle pattern is complex for SSE without SSE4.1,
    // and for F32 data the memory bandwidth is typically the bottleneck anyway.

    // Process 4 pixels at a time (16 floats in, 12 floats out)
    let simd_width = width / 4;
    let remainder = width % 4;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 12;

        dst[dst_offset] = src[src_offset]; // R0
        dst[dst_offset + 1] = src[src_offset + 1]; // G0
        dst[dst_offset + 2] = src[src_offset + 2]; // B0
        dst[dst_offset + 3] = src[src_offset + 4]; // R1
        dst[dst_offset + 4] = src[src_offset + 5]; // G1
        dst[dst_offset + 5] = src[src_offset + 6]; // B1
        dst[dst_offset + 6] = src[src_offset + 8]; // R2
        dst[dst_offset + 7] = src[src_offset + 9]; // G2
        dst[dst_offset + 8] = src[src_offset + 10]; // B2
        dst[dst_offset + 9] = src[src_offset + 12]; // R3
        dst[dst_offset + 10] = src[src_offset + 13]; // G3
        dst[dst_offset + 11] = src[src_offset + 14]; // B3
    }

    // Handle remainder pixels
    for i in 0..remainder {
        let src_idx = simd_width * 16 + i * 4;
        let dst_idx = simd_width * 12 + i * 3;
        dst[dst_idx] = src[src_idx];
        dst[dst_idx + 1] = src[src_idx + 1];
        dst[dst_idx + 2] = src[src_idx + 2];
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn convert_rgb_f32_to_rgba_f32_row_sse(src: &[f32], dst: &mut [f32], width: usize) {
    // F32 channel conversion uses scalar loop with rayon parallelization.
    // The 3<->4 channel shuffle pattern is complex for SSE without SSE4.1,
    // and for F32 data the memory bandwidth is typically the bottleneck anyway.

    // Process 4 pixels at a time (12 floats in, 16 floats out)
    let simd_width = width / 4;
    let remainder = width % 4;

    for i in 0..simd_width {
        let src_offset = i * 12;
        let dst_offset = i * 16;

        dst[dst_offset] = src[src_offset]; // R0
        dst[dst_offset + 1] = src[src_offset + 1]; // G0
        dst[dst_offset + 2] = src[src_offset + 2]; // B0
        dst[dst_offset + 3] = 1.0; // A0

        dst[dst_offset + 4] = src[src_offset + 3]; // R1
        dst[dst_offset + 5] = src[src_offset + 4]; // G1
        dst[dst_offset + 6] = src[src_offset + 5]; // B1
        dst[dst_offset + 7] = 1.0; // A1

        dst[dst_offset + 8] = src[src_offset + 6]; // R2
        dst[dst_offset + 9] = src[src_offset + 7]; // G2
        dst[dst_offset + 10] = src[src_offset + 8]; // B2
        dst[dst_offset + 11] = 1.0; // A2

        dst[dst_offset + 12] = src[src_offset + 9]; // R3
        dst[dst_offset + 13] = src[src_offset + 10]; // G3
        dst[dst_offset + 14] = src[src_offset + 11]; // B3
        dst[dst_offset + 15] = 1.0; // A3
    }

    // Handle remainder pixels
    for i in 0..remainder {
        let src_idx = simd_width * 12 + i * 3;
        let dst_idx = simd_width * 16 + i * 4;
        dst[dst_idx] = src[src_idx];
        dst[dst_idx + 1] = src[src_idx + 1];
        dst[dst_idx + 2] = src[src_idx + 2];
        dst[dst_idx + 3] = 1.0;
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgba_f32_to_rgb_f32_row_neon(src: &[f32], dst: &mut [f32], width: usize) {
    use std::arch::aarch64::*;

    // Process 4 pixels at a time
    let simd_width = width / 4;
    let remainder = width % 4;

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 12;

        unsafe {
            // Load 4 RGBA pixels deinterleaved
            let rgba = vld4q_f32(src.as_ptr().add(src_offset));

            // Store only RGB (3 channels) interleaved
            let rgb = float32x4x3_t(rgba.0, rgba.1, rgba.2);
            vst3q_f32(dst.as_mut_ptr().add(dst_offset), rgb);
        }
    }

    // Handle remainder pixels (scalar)
    for i in 0..remainder {
        let src_idx = simd_width * 16 + i * 4;
        let dst_idx = simd_width * 12 + i * 3;
        dst[dst_idx] = src[src_idx];
        dst[dst_idx + 1] = src[src_idx + 1];
        dst[dst_idx + 2] = src[src_idx + 2];
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn convert_rgb_f32_to_rgba_f32_row_neon(src: &[f32], dst: &mut [f32], width: usize) {
    use std::arch::aarch64::*;

    // Process 4 pixels at a time
    let simd_width = width / 4;
    let remainder = width % 4;

    unsafe {
        let alpha = vdupq_n_f32(1.0);

        for i in 0..simd_width {
            let src_offset = i * 12;
            let dst_offset = i * 16;

            // Load 4 RGB pixels deinterleaved
            let rgb = vld3q_f32(src.as_ptr().add(src_offset));

            // Store RGBA (4 channels) interleaved with alpha = 1.0
            let rgba = float32x4x4_t(rgb.0, rgb.1, rgb.2, alpha);
            vst4q_f32(dst.as_mut_ptr().add(dst_offset), rgba);
        }
    }

    // Handle remainder pixels (scalar)
    for i in 0..remainder {
        let src_idx = simd_width * 12 + i * 3;
        let dst_idx = simd_width * 16 + i * 4;
        dst[dst_idx] = src[src_idx];
        dst[dst_idx + 1] = src[src_idx + 1];
        dst[dst_idx + 2] = src[src_idx + 2];
        dst[dst_idx + 3] = 1.0;
    }
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
// U8 <-> F32 bit depth conversion
// =============================================================================

/// Generic U8 to F32 conversion for any channel count
fn convert_u8_to_f32_generic(from: &Image, to: &mut Image, channels: usize) {
    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;
    let row_bytes = width * channels;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    // Convert to_bytes to f32 slice
    let to_floats: &mut [f32] = bytemuck::cast_slice_mut(to_bytes);
    let to_float_stride = to_stride / 4;

    to_floats
        .par_chunks_mut(to_float_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..y * from_stride + row_bytes];

            #[cfg(target_arch = "x86_64")]
            // SAFETY: We've verified SSE2 is available in try_convert_simd
            unsafe {
                convert_u8_to_f32_row_sse2(from_row, to_row);
            }

            #[cfg(target_arch = "aarch64")]
            // SAFETY: NEON is always available on aarch64
            unsafe {
                convert_u8_to_f32_row_neon(from_row, to_row);
            }
        });
}

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

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row =
                &from_floats[y * from_float_stride..y * from_float_stride + row_elements];

            #[cfg(target_arch = "x86_64")]
            // SAFETY: We've verified SSE2 is available in try_convert_simd
            unsafe {
                convert_f32_to_u8_row_sse2(from_row, to_row);
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
unsafe fn convert_u8_to_f32_row_sse2(src: &[u8], dst: &mut [f32]) {
    use std::arch::x86_64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    let scale = _mm_set1_ps(1.0 / 255.0);

    for i in 0..simd_width {
        let src_offset = i * 16;
        let dst_offset = i * 16;

        unsafe {
            // Load 16 bytes
            let bytes = _mm_loadu_si128(src.as_ptr().add(src_offset) as *const __m128i);

            // Unpack to 16-bit (low 8 bytes)
            let zero = _mm_setzero_si128();
            let words_lo = _mm_unpacklo_epi8(bytes, zero);
            let words_hi = _mm_unpackhi_epi8(bytes, zero);

            // Unpack to 32-bit
            let dwords_0 = _mm_unpacklo_epi16(words_lo, zero);
            let dwords_1 = _mm_unpackhi_epi16(words_lo, zero);
            let dwords_2 = _mm_unpacklo_epi16(words_hi, zero);
            let dwords_3 = _mm_unpackhi_epi16(words_hi, zero);

            // Convert to float and scale
            let floats_0 = _mm_mul_ps(_mm_cvtepi32_ps(dwords_0), scale);
            let floats_1 = _mm_mul_ps(_mm_cvtepi32_ps(dwords_1), scale);
            let floats_2 = _mm_mul_ps(_mm_cvtepi32_ps(dwords_2), scale);
            let floats_3 = _mm_mul_ps(_mm_cvtepi32_ps(dwords_3), scale);

            // Store 16 floats
            _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset), floats_0);
            _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 4), floats_1);
            _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 8), floats_2);
            _mm_storeu_ps(dst.as_mut_ptr().add(dst_offset + 12), floats_3);
        }
    }

    // Handle remainder (scalar)
    for i in 0..remainder {
        dst[simd_width * 16 + i] = src[simd_width * 16 + i] as f32 / 255.0;
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
unsafe fn convert_u8_to_f32_row_neon(src: &[u8], dst: &mut [f32]) {
    use std::arch::aarch64::*;

    let len = src.len();
    let simd_width = len / 16;
    let remainder = len % 16;

    unsafe {
        let scale = vdupq_n_f32(1.0 / 255.0);

        for i in 0..simd_width {
            let src_offset = i * 16;
            let dst_offset = i * 16;

            // Load 16 bytes
            let bytes = vld1q_u8(src.as_ptr().add(src_offset));

            // Split into low and high halves
            let bytes_lo = vget_low_u8(bytes);
            let bytes_hi = vget_high_u8(bytes);

            // Widen to 16-bit
            let words_lo = vmovl_u8(bytes_lo);
            let words_hi = vmovl_u8(bytes_hi);

            // Widen to 32-bit
            let dwords_0 = vmovl_u16(vget_low_u16(words_lo));
            let dwords_1 = vmovl_u16(vget_high_u16(words_lo));
            let dwords_2 = vmovl_u16(vget_low_u16(words_hi));
            let dwords_3 = vmovl_u16(vget_high_u16(words_hi));

            // Convert to float and scale
            let floats_0 = vmulq_f32(vcvtq_f32_u32(dwords_0), scale);
            let floats_1 = vmulq_f32(vcvtq_f32_u32(dwords_1), scale);
            let floats_2 = vmulq_f32(vcvtq_f32_u32(dwords_2), scale);
            let floats_3 = vmulq_f32(vcvtq_f32_u32(dwords_3), scale);

            // Store 16 floats
            vst1q_f32(dst.as_mut_ptr().add(dst_offset), floats_0);
            vst1q_f32(dst.as_mut_ptr().add(dst_offset + 4), floats_1);
            vst1q_f32(dst.as_mut_ptr().add(dst_offset + 8), floats_2);
            vst1q_f32(dst.as_mut_ptr().add(dst_offset + 12), floats_3);
        }
    }

    // Handle remainder (scalar)
    for i in 0..remainder {
        dst[simd_width * 16 + i] = src[simd_width * 16 + i] as f32 / 255.0;
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

    to_words
        .par_chunks_mut(to_word_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..y * from_stride + row_bytes];

            #[cfg(target_arch = "x86_64")]
            // SAFETY: We've verified SSE2 is available in try_convert_simd
            unsafe {
                convert_u8_to_u16_row_sse2(from_row, to_row);
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

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_words[y * from_word_stride..y * from_word_stride + row_elements];

            #[cfg(target_arch = "x86_64")]
            // SAFETY: We've verified SSE2 is available in try_convert_simd
            unsafe {
                convert_u16_to_u8_row_sse2(from_row, to_row);
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

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..];

            #[cfg(target_arch = "x86_64")]
            // SAFETY: We've verified SSSE3 is available in try_convert_simd
            unsafe {
                convert_rgba_to_l_row_ssse3(from_row, to_row, width);
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

    to_bytes
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from_bytes[y * from_stride..];

            #[cfg(target_arch = "x86_64")]
            // SAFETY: We've verified SSSE3 is available in try_convert_simd
            unsafe {
                convert_rgb_to_l_row_ssse3(from_row, to_row, width);
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
