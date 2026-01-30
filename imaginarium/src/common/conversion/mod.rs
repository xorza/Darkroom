pub(crate) mod conversion_scalar;
mod conversion_simd;

#[cfg(test)]
mod simd_tests;
#[cfg(test)]
mod tests;

use common::parallel;
use rayon::prelude::*;

use crate::common::error::Result;
use crate::image::Image;

use conversion_scalar::{ConversionInfo, dispatch_convert_row_scalar};
use conversion_simd::get_simd_row_converter;

#[allow(unused_imports)]
pub use conversion_scalar::OpaqueAlpha;

/// Convert image format, using SIMD acceleration when available.
/// This is the single entry point for all image conversions.
pub fn convert_image(from: &Image, to: &mut Image) -> Result<()> {
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);

    let from_fmt = from.desc().color_format;
    let to_fmt = to.desc().color_format;

    // Same format - nothing to do
    if from_fmt == to_fmt {
        return Ok(());
    }

    let width = from.desc().width;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;

    let from_bytes = from.bytes();
    let to_bytes = to.bytes_mut();

    // Try to get SIMD row converter
    if let Some(simd_convert_row) = get_simd_row_converter(from_fmt, to_fmt) {
        // Use SIMD path with parallel row processing (aligned to row stride)
        parallel::par_chunks_auto_aligned(to_bytes, to_stride).for_each(|(start_row, chunk)| {
            let num_rows = chunk.len() / to_stride;

            for local_row in 0..num_rows {
                let y = start_row + local_row;
                let from_row = &from_bytes[y * from_stride..];
                let to_row = &mut chunk[local_row * to_stride..][..to_stride];
                simd_convert_row(from_row, to_row, width);
            }
        });
    } else {
        // Fall back to scalar path with parallel row processing (aligned to row stride)
        let info = ConversionInfo::new(from_fmt, to_fmt);

        parallel::par_chunks_auto_aligned(to_bytes, to_stride).for_each(|(start_row, chunk)| {
            let num_rows = chunk.len() / to_stride;

            for local_row in 0..num_rows {
                let y = start_row + local_row;
                let from_row = &from_bytes[y * from_stride..];
                let to_row = &mut chunk[local_row * to_stride..][..to_stride];
                dispatch_convert_row_scalar(from_row, to_row, width, &info);
            }
        });
    }

    Ok(())
}
