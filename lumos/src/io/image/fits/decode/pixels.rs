use std::fs::File;
use std::ops::Range;
use std::path::Path;

use fits_well::header::Header;
use fits_well::io::StreamReader;
use rayon::prelude::*;

use common::CancelToken;

use crate::io::image::error::ImageError;
use crate::io::image::fits::decode::DecodedFitsImage;
use crate::io::image::fits::decode::plan::FitsDecodePlan;
use crate::io::image::fits::error::{fits_err, fits_unsupported};
use crate::io::image::fits::metadata::{read_metadata, read_text};
use crate::io::image::fits::provenance::{
    FitsChecksumProvenance, FitsHduProvenance, FitsTransferProvenance,
};
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, LoadContext,
    SourceContainer, TransferProvenance,
};

pub(crate) fn read_stream_hdu(
    reader: &mut StreamReader<File>,
    selected: FitsHduProvenance,
    checksum: FitsChecksumProvenance,
    path: &Path,
    plan: FitsDecodePlan,
    context: &LoadContext,
) -> Result<DecodedFitsImage, ImageError> {
    let index = selected.index;
    let header = reader.hdus()[index].header.clone();
    read_decoded_hdu(&header, plan, selected, checksum, path, context, |ranges| {
        reader
            .read_image_section(index, &ranges)
            .map(|image| image.physical_f32())
    })
}

pub(crate) fn read_decoded_hdu(
    header: &Header,
    plan: FitsDecodePlan,
    hdu: FitsHduProvenance,
    checksum: FitsChecksumProvenance,
    path: &Path,
    context: &LoadContext,
    mut read_pixels: impl FnMut(Vec<Range<usize>>) -> fits_well::Result<Vec<f32>>,
) -> Result<DecodedFitsImage, ImageError> {
    tracing::debug!(
        source_bytes = plan.source_bytes,
        decoded_bytes = plan.decoded_bytes,
        peak_bytes = plan.peak_bytes,
        "FITS image passed header-first memory preflight"
    );
    let pixels = if plan.dimensions.is_rgb() {
        let red = read_fits_plane(path, &plan, 0, context, &mut read_pixels)?;
        let green = read_fits_plane(path, &plan, 1, context, &mut read_pixels)?;
        let blue = read_fits_plane(path, &plan, 2, context, &mut read_pixels)?;
        LinearPixels::from_planar_channels(plan.dimensions, [red, green, blue])
    } else {
        LinearPixels::from_planar_channels(
            plan.dimensions,
            [read_fits_plane(path, &plan, 0, context, &mut read_pixels)?],
        )
    };

    let mut metadata =
        read_metadata(header, plan.shape, plan.bitpix).map_err(|source| fits_err(path, source))?;
    metadata.provenance = Some(ImageProvenance {
        container: SourceContainer::Fits,
        decoder: DecoderProvenance::FitsWell,
        transfer: TransferProvenance::FitsPhysical(FitsTransferProvenance {
            bscale: plan.scaling.bscale,
            bzero: plan.scaling.bzero,
            unit: read_text(header, "BUNIT").map_err(|source| fits_err(path, source))?,
            hdu,
            checksum,
        }),
        color: if metadata.cfa_type.is_some() {
            ColorProvenance::SensorCfa
        } else if plan.dimensions.is_grayscale() {
            ColorProvenance::Monochrome
        } else {
            ColorProvenance::Unspecified
        },
        clipped: false,
        demosaic: DemosaicProvenance::None,
    });

    Ok(DecodedFitsImage { metadata, pixels })
}

fn channel_ranges(plan: &FitsDecodePlan, channel: usize, rows: Range<usize>) -> Vec<Range<usize>> {
    let mut ranges = vec![0..plan.dimensions.width(), rows];
    if plan.shape.len() == 3 {
        ranges.push(channel..channel + 1);
    }
    ranges
}

fn read_fits_plane(
    path: &Path,
    plan: &FitsDecodePlan,
    channel: usize,
    context: &LoadContext,
    read_pixels: &mut impl FnMut(Vec<Range<usize>>) -> fits_well::Result<Vec<f32>>,
) -> Result<Vec<f32>, ImageError> {
    let width = plan.dimensions.width();
    let height = plan.dimensions.height();
    let expected_pixels = plan.dimensions.pixel_count();
    let mut output = vec![0.0; expected_pixels];
    for row_start in (0..height).step_by(plan.rows_per_chunk) {
        context.check_cancelled(path)?;
        let row_end = row_start.saturating_add(plan.rows_per_chunk).min(height);
        let expected_chunk = (row_end - row_start) * width;
        let pixels = read_pixels(channel_ranges(plan, channel, row_start..row_end))
            .map_err(|source| fits_err(path, source))?;
        context.check_cancelled(path)?;
        if pixels.len() != expected_chunk {
            return Err(fits_unsupported(
                path,
                format!(
                    "channel {channel} rows {row_start}..{row_end} contain {} pixels; expected {expected_chunk}",
                    pixels.len()
                ),
            ));
        }
        let pixels = validate_fits_pixels(pixels, &context.cancel).map_err(|error| match error {
            PixelValidationError::Cancelled => ImageError::Cancelled {
                path: path.to_path_buf(),
            },
            PixelValidationError::Nulls(summary) => fits_unsupported(
                path,
                format!(
                    "image contains {} null/non-finite pixels in a decode chunk; first at linear index {}",
                    summary.count,
                    channel * expected_pixels + row_start * width + summary.first_index
                ),
            ),
        })?;
        let start = row_start * width;
        output[start..start + expected_chunk].copy_from_slice(&pixels);
    }
    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NullSummary {
    count: usize,
    first_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PixelValidationError {
    Cancelled,
    Nulls(NullSummary),
}

fn validate_fits_pixels(
    pixels: Vec<f32>,
    cancel: &CancelToken,
) -> Result<Vec<f32>, PixelValidationError> {
    const VALIDATION_CHUNK_SAMPLES: usize = 64 * 1024;
    if cancel.is_cancelled() {
        return Err(PixelValidationError::Cancelled);
    }
    let nulls = pixels
        .par_chunks(VALIDATION_CHUNK_SAMPLES)
        .enumerate()
        .map(|(chunk_index, chunk)| {
            if cancel.is_cancelled() {
                return Err(PixelValidationError::Cancelled);
            }
            let chunk_start = chunk_index * VALIDATION_CHUNK_SAMPLES;
            Ok(chunk
                .iter()
                .enumerate()
                .filter_map(|(index, pixel)| {
                    (!pixel.is_finite()).then_some(NullSummary {
                        count: 1,
                        first_index: chunk_start + index,
                    })
                })
                .reduce(|left, right| NullSummary {
                    count: left.count + right.count,
                    first_index: left.first_index.min(right.first_index),
                }))
        })
        .try_reduce(
            || None,
            |left, right| {
                Ok(match (left, right) {
                    (None, value) | (value, None) => value,
                    (Some(left), Some(right)) => Some(NullSummary {
                        count: left.count + right.count,
                        first_index: left.first_index.min(right.first_index),
                    }),
                })
            },
        )?;
    if let Some(summary) = nulls {
        return Err(PixelValidationError::Nulls(summary));
    }

    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use common::CancelToken;

    use crate::io::image::fits::decode::pixels::{
        NullSummary, PixelValidationError, validate_fits_pixels,
    };

    #[test]
    fn cancellation_stops_chunk_validation() {
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            validate_fits_pixels(vec![1.0, 2.0], &cancel).unwrap_err(),
            PixelValidationError::Cancelled
        );
    }

    #[test]
    fn finite_validation_preserves_every_physical_value() {
        let pixels = vec![-5.0, 0.0, 0.5, 2.0, 255.0, 65_535.0];
        assert_eq!(
            validate_fits_pixels(pixels.clone(), &CancelToken::never()).unwrap(),
            pixels
        );
    }

    #[test]
    fn non_finite_pixels_return_exact_summary_for_every_sample_type() {
        let pixels = vec![0.0, f32::NAN, 5.0, f32::INFINITY, f32::NEG_INFINITY];

        assert_eq!(
            validate_fits_pixels(pixels, &CancelToken::never()).unwrap_err(),
            PixelValidationError::Nulls(NullSummary {
                count: 3,
                first_index: 1,
            })
        );
    }
}
