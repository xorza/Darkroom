//! Format conversion — change an interleaved [`Image`]'s pixel *format* (element
//! type `u8`/`u16`/`f32` and/or channel count L/RGB/RGBA), e.g. `RGB_U8` →
//! `RGBA_F32`. [`convert_image`] is the single entry point: it processes rows in
//! parallel through a SIMD kernel when one exists for the pair ([`simd`]), else
//! the scalar reference ([`scalar`]). Layout is preserved (interleaved →
//! interleaved); changing *layout* is the separate
//! [`transpose`](crate::image::transpose).

pub(crate) mod scalar;
pub(crate) mod simd;

// Rec. 709 (sRGB) luminance weights scaled to fixed-point for integer math
// R: 0.2126 * 65536 = 13933
// G: 0.7152 * 65536 = 46871
// B: 0.0722 * 65536 = 4732
// Total: 65536 (allows shift by 16 instead of divide)
pub(crate) const LUMA_R: u32 = 13933;
pub(crate) const LUMA_G: u32 = 46871;
pub(crate) const LUMA_B: u32 = 4732;

// Rec. 709 weights scaled to 8-bit fixed point (sum 256, allows >>8 instead of divide).
// Used by the SIMD integer luminance kernels; [R, G, B].
pub(crate) const LUMA_8BIT: [u16; 3] = [54, 183, 19];

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use rayon::prelude::*;

use crate::image::Image;

use scalar::{ConversionInfo, dispatch_convert_row_scalar};
use simd::get_simd_row_converter;

/// Convert `from` into `to`'s format, using SIMD acceleration when available.
/// `from` and `to` must share dimensions; every format pair is handled (SIMD
/// fast paths fall back to the scalar reference), so this is infallible.
pub(crate) fn convert_image(from: &Image, to: &mut Image) {
    assert_eq!(from.desc().width, to.desc().width);
    assert_eq!(from.desc().height, to.desc().height);

    let from_fmt = from.desc().color_format;
    let to_fmt = to.desc().color_format;

    // Same format - nothing to do
    if from_fmt == to_fmt {
        return;
    }

    let width = from.desc().width;
    let from_stride = from.desc().row_bytes();
    let to_stride = to.desc().row_bytes();

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    // Try to get SIMD row converter
    if let Some(simd_convert_row) = get_simd_row_converter(from_fmt, to_fmt) {
        // Use SIMD path with parallel row processing
        to_bytes
            .par_chunks_mut(to_stride)
            .enumerate()
            .for_each(|(y, to_row)| {
                let from_row = &from_bytes[y * from_stride..];
                simd_convert_row(from_row, to_row, width);
            });
    } else {
        // Fall back to scalar path with parallel row processing
        let info = ConversionInfo::new(from_fmt, to_fmt);

        to_bytes
            .par_chunks_mut(to_stride)
            .enumerate()
            .for_each(|(y, to_row)| {
                let from_row = &from_bytes[y * from_stride..];
                dispatch_convert_row_scalar(from_row, to_row, width, &info);
            });
    }
}
