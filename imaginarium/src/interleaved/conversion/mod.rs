//! Format conversion — change an [`InterleavedImage`]'s pixel *format* (element
//! type `u8`/`u16`/`f32` and/or channel count mono/RGB/RGBA), e.g. `RGB_U8` →
//! `RGBA_F32`. [`convert_image`] is the single entry point: it processes rows in
//! parallel through a vector kernel when the pair has one ([`VectorConversion`]), else
//! the scalar reference ([`scalar`]).

pub(crate) mod channels;
pub(crate) mod elements;
pub(crate) mod scalar;
pub(crate) mod vector_conversion;

use common::par;
use rayon::prelude::*;
use simd::IsaTier;

use crate::interleaved::conversion::vector_conversion::VectorConversion;
use crate::interleaved::interleaved_image::InterleavedImage;

/// Convert `from` into `to`'s format, through a vector kernel when the pair has one.
///
/// # Panics
/// Unless the two images share dimensions and differ in format — an equal format is a copy,
/// which the caller makes.
pub(crate) fn convert_image(from: &InterleavedImage, to: &mut InterleavedImage) {
    let (from_desc, to_desc) = (from.desc(), to.desc());
    assert_eq!(
        (from_desc.size().width, from_desc.size().height),
        (to_desc.size().width, to_desc.size().height),
        "source/target dimensions mismatch"
    );
    assert_ne!(
        from_desc.color_format(),
        to_desc.color_format(),
        "a conversion to the same format is a copy"
    );

    let width = from_desc.size().width;
    let from_stride = from_desc.row_byte_len();
    let to_stride = to_desc.row_byte_len();
    let from_bytes = from.bytes();
    let rows = par::chunks_mut(to.bytes_mut(), to_stride).enumerate();

    let (from_format, to_format) = (from_desc.color_format(), to_desc.color_format());
    if let Some(kernel) = VectorConversion::of(from_format, to_format) {
        let tier = IsaTier::widest();
        rows.for_each(|(y, to_row)| {
            kernel.convert_row(tier, &from_bytes[y * from_stride..], to_row, width);
        });
    } else {
        let convert_row = scalar::row_converter(from_format, to_format);
        rows.for_each(|(y, to_row)| convert_row(&from_bytes[y * from_stride..], to_row, width));
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
