//! Format conversion — change an [`InterleavedImage`]'s pixel *format* (element
//! type `u8`/`u16`/`f32` and/or channel count mono/RGB/RGBA), e.g. `RGB_U8` →
//! `RGBA_F32`. [`convert_image`] is the single entry point: it processes rows in
//! parallel through a SIMD kernel when one exists for the pair ([`simd`]), else
//! the scalar reference ([`scalar`]).

pub(crate) mod scalar;
pub(crate) mod simd;

use common::par;
use rayon::prelude::*;

use crate::image::InterleavedImage;

/// Convert `from` into `to`'s format, using SIMD acceleration when available.
///
/// # Panics
/// Unless the two images share dimensions and differ in format — an equal format is a copy,
/// which the caller makes.
pub(crate) fn convert_image(from: &InterleavedImage, to: &mut InterleavedImage) {
    let (from_desc, to_desc) = (from.desc(), to.desc());
    assert_eq!(
        (from_desc.size.width, from_desc.size.height),
        (to_desc.size.width, to_desc.size.height),
        "source/target dimensions mismatch"
    );
    assert_ne!(
        from_desc.color_format, to_desc.color_format,
        "a conversion to the same format is a copy"
    );

    let width = from_desc.size.width;
    let from_stride = from_desc.row_byte_len();
    let to_stride = to_desc.row_byte_len();
    let from_bytes = from.bytes();
    let rows = par::chunks_mut(to.bytes_mut(), to_stride).enumerate();

    if let Some(kernel) = simd::row_converter(from_desc.color_format, to_desc.color_format) {
        rows.for_each(|(y, to_row)| {
            unsafe { kernel(&from_bytes[y * from_stride..], to_row, width) };
        });
    } else {
        let convert_row = scalar::row_converter(from_desc.color_format, to_desc.color_format);
        rows.for_each(|(y, to_row)| convert_row(&from_bytes[y * from_stride..], to_row, width));
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
