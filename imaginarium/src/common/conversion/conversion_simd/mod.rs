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
use common::{cfg_aarch64, cfg_x86_64, cpu_features};

cfg_x86_64! {
    pub(crate) mod avx;
    pub(crate) mod sse;
}

cfg_aarch64! {
    pub(crate) mod neon;
}

#[cfg(test)]
mod tests;

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
    cfg_x86_64! {
        fn get_x86_64(from_fmt: ColorFormat, to_fmt: ColorFormat) -> Option<RowConvertFn> {
            let features = cpu_features::get();
            if !features.sse2 {
                return None;
            }

            match (from_fmt, to_fmt) {
                // Channel conversions U8 (require SSSE3)
                (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) if features.ssse3 => {
                    Some(convert_rgba_u8_to_rgb_u8_row)
                }
                (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) if features.ssse3 => {
                    Some(convert_rgb_u8_to_rgba_u8_row)
                }
                // Luminance U8 (require SSSE3)
                (ColorFormat::RGBA_U8, ColorFormat::L_U8) if features.ssse3 => {
                    Some(convert_rgba_u8_to_l_u8_row)
                }
                (ColorFormat::RGB_U8, ColorFormat::L_U8) if features.ssse3 => {
                    Some(convert_rgb_u8_to_l_u8_row)
                }
                // L_U8 expansion (require SSSE3)
                (ColorFormat::L_U8, ColorFormat::RGBA_U8) if features.ssse3 => {
                    Some(convert_l_u8_to_rgba_u8_row)
                }
                (ColorFormat::L_U8, ColorFormat::RGB_U8) if features.ssse3 => {
                    Some(convert_l_u8_to_rgb_u8_row)
                }
                // LA_U8 <-> RGBA_U8 (require SSSE3)
                (ColorFormat::LA_U8, ColorFormat::RGBA_U8) if features.ssse3 => {
                    Some(convert_la_u8_to_rgba_u8_row)
                }
                (ColorFormat::RGBA_U8, ColorFormat::LA_U8) if features.ssse3 => {
                    Some(convert_rgba_u8_to_la_u8_row)
                }
                // F32<->U8
                (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => Some(convert_f32_to_u8_row_4ch),
                (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => Some(convert_f32_to_u8_row_3ch),
                (ColorFormat::L_F32, ColorFormat::L_U8) => Some(convert_f32_to_u8_row_1ch),
                (ColorFormat::LA_F32, ColorFormat::LA_U8) => Some(convert_f32_to_u8_row_2ch),
                (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32) => Some(convert_u8_to_f32_row_4ch),
                (ColorFormat::RGB_U8, ColorFormat::RGB_F32) => Some(convert_u8_to_f32_row_3ch),
                (ColorFormat::L_U8, ColorFormat::L_F32) => Some(convert_u8_to_f32_row_1ch),
                (ColorFormat::LA_U8, ColorFormat::LA_F32) => Some(convert_u8_to_f32_row_2ch),
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
                (ColorFormat::RGB_U16, ColorFormat::RGB_F32) => Some(convert_u16_to_f32_row_3ch),
                (ColorFormat::RGBA_U16, ColorFormat::RGBA_F32) => Some(convert_u16_to_f32_row_4ch),
                _ => None,
            }
        }
    }

    cfg_aarch64! {
        fn get_aarch64(from_fmt: ColorFormat, to_fmt: ColorFormat) -> Option<RowConvertFn> {
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
                // F32<->U8
                (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => Some(convert_f32_to_u8_row_4ch),
                (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => Some(convert_f32_to_u8_row_3ch),
                (ColorFormat::L_F32, ColorFormat::L_U8) => Some(convert_f32_to_u8_row_1ch),
                (ColorFormat::LA_F32, ColorFormat::LA_U8) => Some(convert_f32_to_u8_row_2ch),
                (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32) => Some(convert_u8_to_f32_row_4ch),
                (ColorFormat::RGB_U8, ColorFormat::RGB_F32) => Some(convert_u8_to_f32_row_3ch),
                (ColorFormat::L_U8, ColorFormat::L_F32) => Some(convert_u8_to_f32_row_1ch),
                (ColorFormat::LA_U8, ColorFormat::LA_F32) => Some(convert_u8_to_f32_row_2ch),
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
                (ColorFormat::RGB_U16, ColorFormat::RGB_F32) => Some(convert_u16_to_f32_row_3ch),
                (ColorFormat::RGBA_U16, ColorFormat::RGBA_F32) => Some(convert_u16_to_f32_row_4ch),
                _ => None,
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    return get_x86_64(from_fmt, to_fmt);

    #[cfg(target_arch = "aarch64")]
    return get_aarch64(from_fmt, to_fmt);

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
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u8], dst: &mut [u8], width: usize) {
            if cpu_features::has_avx2() {
                avx::convert_rgba_to_rgb_row_avx2(src, dst, width);
            } else {
                sse::convert_rgba_to_rgb_row_ssse3(src, dst, width);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u8], dst: &mut [u8], width: usize) {
            neon::convert_rgba_to_rgb_row_neon(src, dst, width);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst, width)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst, width)
    }
}

fn convert_rgb_u8_to_rgba_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u8], dst: &mut [u8], width: usize) {
            if cpu_features::has_avx2() {
                avx::convert_rgb_to_rgba_row_avx2(src, dst, width);
            } else {
                sse::convert_rgb_to_rgba_row_ssse3(src, dst, width);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u8], dst: &mut [u8], width: usize) {
            neon::convert_rgb_to_rgba_row_neon(src, dst, width);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst, width)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst, width)
    }
}

fn convert_rgba_u8_to_l_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u8], dst: &mut [u8], width: usize) {
            if cpu_features::has_avx2() {
                avx::convert_rgba_to_l_row_avx2(src, dst, width);
            } else {
                sse::convert_rgba_to_l_row_ssse3(src, dst, width);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u8], dst: &mut [u8], width: usize) {
            neon::convert_rgba_to_l_row_neon(src, dst, width);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst, width)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst, width)
    }
}

fn convert_rgb_u8_to_l_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u8], dst: &mut [u8], width: usize) {
            if cpu_features::has_avx2() {
                avx::convert_rgb_to_l_row_avx2(src, dst, width);
            } else {
                sse::convert_rgb_to_l_row_ssse3(src, dst, width);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u8], dst: &mut [u8], width: usize) {
            neon::convert_rgb_to_l_row_neon(src, dst, width);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst, width)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst, width)
    }
}

fn convert_l_u8_to_rgba_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u8], dst: &mut [u8], width: usize) {
            if cpu_features::has_avx2() {
                avx::convert_l_to_rgba_row_avx2(src, dst, width);
            } else {
                sse::convert_l_to_rgba_row_ssse3(src, dst, width);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u8], dst: &mut [u8], width: usize) {
            neon::convert_l_to_rgba_row_neon(src, dst, width);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst, width)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst, width)
    }
}

fn convert_l_u8_to_rgb_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u8], dst: &mut [u8], width: usize) {
            if cpu_features::has_avx2() {
                avx::convert_l_to_rgb_row_avx2(src, dst, width);
            } else {
                sse::convert_l_to_rgb_row_ssse3(src, dst, width);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u8], dst: &mut [u8], width: usize) {
            neon::convert_l_to_rgb_row_neon(src, dst, width);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst, width)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst, width)
    }
}

fn convert_la_u8_to_rgba_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u8], dst: &mut [u8], width: usize) {
            if cpu_features::has_avx2() {
                avx::convert_la_to_rgba_row_avx2(src, dst, width);
            } else {
                sse::convert_la_to_rgba_row_ssse3(src, dst, width);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u8], dst: &mut [u8], width: usize) {
            neon::convert_la_to_rgba_row_neon(src, dst, width);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst, width)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst, width)
    }
}

fn convert_rgba_u8_to_la_u8_row(src: &[u8], dst: &mut [u8], width: usize) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u8], dst: &mut [u8], width: usize) {
            if cpu_features::has_avx2() {
                avx::convert_rgba_to_la_row_avx2(src, dst, width);
            } else {
                sse::convert_rgba_to_la_row_ssse3(src, dst, width);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u8], dst: &mut [u8], width: usize) {
            neon::convert_rgba_to_la_row_neon(src, dst, width);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst, width)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst, width)
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

// Channel-specific U8->F32 wrappers
fn convert_u8_to_f32_row_1ch(src: &[u8], dst: &mut [u8], width: usize) {
    let dst_floats: &mut [f32] = bytemuck::cast_slice_mut(&mut dst[..width * 4]);
    convert_u8_to_f32_row(&src[..width], dst_floats);
}

fn convert_u8_to_f32_row_2ch(src: &[u8], dst: &mut [u8], width: usize) {
    let dst_floats: &mut [f32] = bytemuck::cast_slice_mut(&mut dst[..width * 8]);
    convert_u8_to_f32_row(&src[..width * 2], dst_floats);
}

fn convert_u8_to_f32_row_3ch(src: &[u8], dst: &mut [u8], width: usize) {
    let dst_floats: &mut [f32] = bytemuck::cast_slice_mut(&mut dst[..width * 12]);
    convert_u8_to_f32_row(&src[..width * 3], dst_floats);
}

fn convert_u8_to_f32_row_4ch(src: &[u8], dst: &mut [u8], width: usize) {
    let dst_floats: &mut [f32] = bytemuck::cast_slice_mut(&mut dst[..width * 16]);
    convert_u8_to_f32_row(&src[..width * 4], dst_floats);
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

fn convert_u16_to_f32_row_3ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_words: &[u16] = bytemuck::cast_slice(&src[..width * 6]);
    let dst_floats: &mut [f32] = bytemuck::cast_slice_mut(&mut dst[..width * 12]);
    convert_u16_to_f32_row(src_words, dst_floats);
}

fn convert_u16_to_f32_row_4ch(src: &[u8], dst: &mut [u8], width: usize) {
    let src_words: &[u16] = bytemuck::cast_slice(&src[..width * 8]);
    let dst_floats: &mut [f32] = bytemuck::cast_slice_mut(&mut dst[..width * 16]);
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
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[f32], dst: &mut [u8]) {
            if cpu_features::has_avx2() {
                avx::convert_f32_to_u8_row_avx2(src, dst);
            } else {
                sse::convert_f32_to_u8_row_sse2(src, dst);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[f32], dst: &mut [u8]) {
            neon::convert_f32_to_u8_row_neon(src, dst);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst)
    }
}

fn convert_u8_to_f32_row(src: &[u8], dst: &mut [f32]) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u8], dst: &mut [f32]) {
            if cpu_features::has_avx2() {
                avx::convert_u8_to_f32_row_avx2(src, dst);
            } else {
                sse::convert_u8_to_f32_row_sse2(src, dst);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u8], dst: &mut [f32]) {
            neon::convert_u8_to_f32_row_neon(src, dst);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst)
    }
}

fn convert_u8_to_u16_row(src: &[u8], dst: &mut [u16]) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u8], dst: &mut [u16]) {
            if cpu_features::has_avx2() {
                avx::convert_u8_to_u16_row_avx2(src, dst);
            } else {
                sse::convert_u8_to_u16_row_sse2(src, dst);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u8], dst: &mut [u16]) {
            neon::convert_u8_to_u16_row_neon(src, dst);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst)
    }
}

fn convert_u16_to_u8_row(src: &[u16], dst: &mut [u8]) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u16], dst: &mut [u8]) {
            if cpu_features::has_avx2() {
                avx::convert_u16_to_u8_row_avx2(src, dst);
            } else {
                sse::convert_u16_to_u8_row_sse2(src, dst);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u16], dst: &mut [u8]) {
            neon::convert_u16_to_u8_row_neon(src, dst);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst)
    }
}

fn convert_u16_to_f32_row(src: &[u16], dst: &mut [f32]) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[u16], dst: &mut [f32]) {
            if cpu_features::has_avx2() {
                avx::convert_u16_to_f32_row_avx2(src, dst);
            } else {
                sse::convert_u16_to_f32_row_sse2(src, dst);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[u16], dst: &mut [f32]) {
            neon::convert_u16_to_f32_row_neon(src, dst);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst)
    }
}

fn convert_f32_to_u16_row(src: &[f32], dst: &mut [u16]) {
    cfg_x86_64! {
        unsafe fn impl_x86_64(src: &[f32], dst: &mut [u16]) {
            if cpu_features::has_avx2() {
                avx::convert_f32_to_u16_row_avx2(src, dst);
            } else {
                sse::convert_f32_to_u16_row_sse2(src, dst);
            }
        }
    }
    cfg_aarch64! {
        unsafe fn impl_aarch64(src: &[f32], dst: &mut [u16]) {
            neon::convert_f32_to_u16_row_neon(src, dst);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe {
        impl_x86_64(src, dst)
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        impl_aarch64(src, dst)
    }
}
