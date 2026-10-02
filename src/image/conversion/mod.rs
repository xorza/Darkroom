//! Format conversion — change an interleaved [`Image`]'s pixel *format* (element
//! type `u8`/`u16`/`f32` and/or channel count L/RGB/RGBA), e.g. `RGB_U8` →
//! `RGBA_F32`. [`convert_image`] is the single entry point: it processes rows in
//! parallel through a SIMD kernel when one exists for the pair ([`simd`]), else
//! the scalar reference ([`scalar`]). Layout is preserved (interleaved →
//! interleaved); changing *layout* is the separate
//! [`transpose`](crate::image::transpose).

pub(crate) mod scalar;
pub(crate) mod simd;

use rayon::prelude::*;

use crate::image::Image;

/// Convert `from` into `to`'s format, using SIMD acceleration when available.
///
/// # Panics
/// Unless the two images share dimensions and differ in format — an equal format is a copy,
/// which the caller makes.
pub(crate) fn convert_image(from: &Image, to: &mut Image) {
    let (from_desc, to_desc) = (from.desc(), to.desc());
    assert_eq!(
        (from_desc.width, from_desc.height),
        (to_desc.width, to_desc.height),
        "source/target dimensions mismatch"
    );
    assert_ne!(
        from_desc.color_format, to_desc.color_format,
        "a conversion to the same format is a copy"
    );

    let width = from_desc.width;
    let from_stride = from_desc.row_bytes();
    let to_stride = to_desc.row_bytes();
    let from_bytes = from.bytes();
    let rows = to.bytes_mut().par_chunks_mut(to_stride).enumerate();

    if let Some(kernel) = simd::row_converter(from_desc.color_format, to_desc.color_format) {
        rows.for_each(|(y, to_row)| {
            // SAFETY: `row_converter` verified this CPU has the kernel's feature.
            unsafe { kernel(&from_bytes[y * from_stride..], to_row, width) };
        });
    } else {
        let convert_row = scalar::row_converter(from_desc.color_format, to_desc.color_format);
        rows.for_each(|(y, to_row)| convert_row(&from_bytes[y * from_stride..], to_row, width));
    }
}

#[cfg(feature = "bench")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
