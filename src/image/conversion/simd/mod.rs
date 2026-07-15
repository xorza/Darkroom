//! SIMD implementations for single-row conversion.

#![allow(unsafe_op_in_unsafe_fn)]

use crate::common::color_format::ColorFormat;
#[cfg(target_arch = "x86_64")]
use crate::cpu_features;

cfg_x86_64! {
    pub(crate) mod avx;
    pub(crate) mod sse;
}

cfg_aarch64! {
    pub(crate) mod neon;
}

#[cfg(test)]
mod tests;

use super::{LUMA_8BIT, LUMA_B, LUMA_G, LUMA_R};

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
                // F32<->U8
                (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => Some(convert_f32_to_u8_bytes),
                (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => Some(convert_f32_to_u8_bytes),
                (ColorFormat::L_F32, ColorFormat::L_U8) => Some(convert_f32_to_u8_bytes),
                (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32) => Some(convert_u8_to_f32_bytes),
                (ColorFormat::RGB_U8, ColorFormat::RGB_F32) => Some(convert_u8_to_f32_bytes),
                (ColorFormat::L_U8, ColorFormat::L_F32) => Some(convert_u8_to_f32_bytes),
                // U8<->U16
                (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16) => Some(convert_u8_to_u16_bytes),
                (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8) => Some(convert_u16_to_u8_bytes),
                (ColorFormat::RGB_U8, ColorFormat::RGB_U16) => Some(convert_u8_to_u16_bytes),
                (ColorFormat::RGB_U16, ColorFormat::RGB_U8) => Some(convert_u16_to_u8_bytes),
                (ColorFormat::L_U8, ColorFormat::L_U16) => Some(convert_u8_to_u16_bytes),
                (ColorFormat::L_U16, ColorFormat::L_U8) => Some(convert_u16_to_u8_bytes),
                // U16<->F32
                (ColorFormat::L_U16, ColorFormat::L_F32) => Some(convert_u16_to_f32_bytes),
                (ColorFormat::L_F32, ColorFormat::L_U16) => Some(convert_f32_to_u16_bytes),
                (ColorFormat::RGB_U16, ColorFormat::RGB_F32) => Some(convert_u16_to_f32_bytes),
                (ColorFormat::RGBA_U16, ColorFormat::RGBA_F32) => Some(convert_u16_to_f32_bytes),
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
                // F32<->U8
                (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8) => Some(convert_f32_to_u8_bytes),
                (ColorFormat::RGB_F32, ColorFormat::RGB_U8) => Some(convert_f32_to_u8_bytes),
                (ColorFormat::L_F32, ColorFormat::L_U8) => Some(convert_f32_to_u8_bytes),
                (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32) => Some(convert_u8_to_f32_bytes),
                (ColorFormat::RGB_U8, ColorFormat::RGB_F32) => Some(convert_u8_to_f32_bytes),
                (ColorFormat::L_U8, ColorFormat::L_F32) => Some(convert_u8_to_f32_bytes),
                // U8<->U16
                (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16) => Some(convert_u8_to_u16_bytes),
                (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8) => Some(convert_u16_to_u8_bytes),
                (ColorFormat::RGB_U8, ColorFormat::RGB_U16) => Some(convert_u8_to_u16_bytes),
                (ColorFormat::RGB_U16, ColorFormat::RGB_U8) => Some(convert_u16_to_u8_bytes),
                (ColorFormat::L_U8, ColorFormat::L_U16) => Some(convert_u8_to_u16_bytes),
                (ColorFormat::L_U16, ColorFormat::L_U8) => Some(convert_u16_to_u8_bytes),
                // U16<->F32
                (ColorFormat::L_U16, ColorFormat::L_F32) => Some(convert_u16_to_f32_bytes),
                (ColorFormat::L_F32, ColorFormat::L_U16) => Some(convert_f32_to_u16_bytes),
                (ColorFormat::RGB_U16, ColorFormat::RGB_F32) => Some(convert_u16_to_f32_bytes),
                (ColorFormat::RGBA_U16, ColorFormat::RGBA_F32) => Some(convert_u16_to_f32_bytes),
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
            sse::convert_rgb_to_rgba_row_ssse3(src, dst, width);
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
            sse::convert_rgba_to_l_row_ssse3(src, dst, width);
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
            sse::convert_rgb_to_l_row_ssse3(src, dst, width);
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
            sse::convert_l_to_rgba_row_ssse3(src, dst, width);
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
            sse::convert_l_to_rgb_row_ssse3(src, dst, width);
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

// Element-type converters: change the sample type, channel count unchanged.
// Element conversion is per-sample (channel-agnostic), so one wrapper per
// type-pair handles L/RGB/RGBA alike — the sample count is just
// `dst.len() / size_of::<Dst>()` (each `to_row` is exactly one packed row).
// `_width` is unused here; the shared `RowConvertFn` signature carries it for the
// channel-changing kernels above.
macro_rules! elem_row {
    ($name:ident, $core:ident, $src:ty => $dst:ty) => {
        fn $name(src: &[u8], dst: &mut [u8], _width: usize) {
            let count = dst.len() / std::mem::size_of::<$dst>();
            let src_typed: &[$src] =
                bytemuck::cast_slice(&src[..count * std::mem::size_of::<$src>()]);
            let dst_typed: &mut [$dst] = bytemuck::cast_slice_mut(dst);
            $core(src_typed, dst_typed);
        }
    };
}

elem_row!(convert_f32_to_u8_bytes, convert_f32_to_u8_row, f32 => u8);
elem_row!(convert_u8_to_f32_bytes, convert_u8_to_f32_row, u8 => f32);
elem_row!(convert_u8_to_u16_bytes, convert_u8_to_u16_row, u8 => u16);
elem_row!(convert_u16_to_u8_bytes, convert_u16_to_u8_row, u16 => u8);
elem_row!(convert_u16_to_f32_bytes, convert_u16_to_f32_row, u16 => f32);
elem_row!(convert_f32_to_u16_bytes, convert_f32_to_u16_row, f32 => u16);

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
            } else if cpu_features::has_sse4_1() {
                sse::convert_f32_to_u16_row_sse41(src, dst);
            } else {
                for (s, d) in src.iter().zip(dst.iter_mut()) {
                    *d = (*s * 65535.0).clamp(0.0, 65535.0) as u16;
                }
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
