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
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, SourceContainer,
    TransferProvenance,
};
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::load_context::LoadContext;

pub(super) fn read_stream_hdu(
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

pub(super) fn read_decoded_hdu(
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
    // DATAMAX is a saturation level in the file's sample units, so it only stays comparable to the
    // samples if it is divided by the same span they were.
    if let Some(data_max) = &mut metadata.data_max {
        *data_max /= f64::from(plan.sample_divisor);
    }
    metadata.provenance = Some(ImageProvenance {
        container: SourceContainer::Fits,
        decoder: DecoderProvenance::FitsWell,
        transfer: TransferProvenance::FitsNormalized(FitsTransferProvenance {
            bscale: plan.scaling.bscale,
            bzero: plan.scaling.bzero,
            physical_scale: plan.sample_divisor,
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
        let mut pixels = pixels;
        validate_and_normalize(&mut pixels, plan.sample_divisor, &context.cancel).map_err(
            |error| match error {
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
            },
        )?;
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

/// Reject a chunk's FITS nulls, then divide it into the pipeline's `[0, 1]` domain.
///
/// One pass rather than two: this already walks every sample of every plane of every frame, and
/// the scale is a multiply on a value the validation just loaded. Validation runs first per
/// element, so `divisor` never turns a rejected null into a finite-looking sample.
///
/// A `divisor` of 1 — every floating-point FITS, see [`super::plan`] — skips the scaling entirely
/// rather than multiplying by one.
fn validate_and_normalize(
    pixels: &mut [f32],
    divisor: f32,
    cancel: &CancelToken,
) -> Result<(), PixelValidationError> {
    const VALIDATION_CHUNK_SAMPLES: usize = 64 * 1024;
    if cancel.is_cancelled() {
        return Err(PixelValidationError::Cancelled);
    }
    let scale = divisor != 1.0;
    let nulls = pixels
        .par_chunks_mut(VALIDATION_CHUNK_SAMPLES)
        .enumerate()
        .map(|(chunk_index, chunk)| {
            if cancel.is_cancelled() {
                return Err(PixelValidationError::Cancelled);
            }
            let chunk_start = chunk_index * VALIDATION_CHUNK_SAMPLES;
            let mut nulls: Option<NullSummary> = None;
            for (index, pixel) in chunk.iter_mut().enumerate() {
                if !pixel.is_finite() {
                    let found = NullSummary {
                        count: 1,
                        first_index: chunk_start + index,
                    };
                    nulls = Some(match nulls {
                        None => found,
                        Some(seen) => NullSummary {
                            count: seen.count + 1,
                            first_index: seen.first_index.min(found.first_index),
                        },
                    });
                    continue;
                }
                if scale {
                    // Divide rather than multiply by a reciprocal: the reciprocal of a divisor
                    // like 65535 is inexact in f32, and the pipeline's contract is precision
                    // ahead of throughput. The cost hides behind this pass's memory traffic.
                    *pixel /= divisor;
                }
            }
            Ok(nulls)
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use common::CancelToken;

    use crate::io::image::fits::decode::pixels::{
        NullSummary, PixelValidationError, validate_and_normalize,
    };

    #[test]
    fn cancellation_stops_chunk_validation() {
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            validate_and_normalize(&mut [1.0, 2.0], 1.0, &cancel).unwrap_err(),
            PixelValidationError::Cancelled
        );
    }

    #[test]
    fn a_unit_divisor_preserves_every_physical_value() {
        // The float-FITS path: no declared full scale, so nothing is divided.
        let mut pixels = [-5.0, 0.0, 0.5, 2.0, 255.0, 65_535.0];
        let expected = pixels;
        validate_and_normalize(&mut pixels, 1.0, &CancelToken::never()).unwrap();
        assert_eq!(pixels, expected);
    }

    #[test]
    fn an_integer_divisor_maps_the_declared_span_onto_the_unit_interval() {
        // BITPIX = 16 with BZERO = 2¹⁵, BSCALE = 1: divisor |1| × (2¹⁶ − 1) = 65535, so the
        // unsigned span 0..=65535 lands exactly on [0, 1].
        let mut unsigned = [0.0, 16_384.0, 32_768.0, 65_535.0];
        validate_and_normalize(&mut unsigned, 65_535.0, &CancelToken::never()).unwrap();
        // The endpoints are exact; 16384/65535 = 0.2500038147, 32768/65535 = 0.5000076294.
        assert_eq!(unsigned[0], 0.0);
        assert!((unsigned[1] - 0.250_003_8).abs() < 1e-7, "{unsigned:?}");
        assert!((unsigned[2] - 0.500_007_6).abs() < 1e-7, "{unsigned:?}");
        assert_eq!(unsigned[3], 1.0);

        // The same divisor puts a signed frame on [-0.5, 0.5] around its own zero: the scale is
        // applied without an offset, so a negative sample stays negative.
        let mut signed = [-32_768.0, 0.0, 32_767.0];
        validate_and_normalize(&mut signed, 65_535.0, &CancelToken::never()).unwrap();
        assert!((signed[0] - -0.500_007_6).abs() < 1e-7, "{signed:?}");
        assert_eq!(signed[1], 0.0);
        assert!((signed[2] - 0.499_992_37).abs() < 1e-7, "{signed:?}");
    }

    #[test]
    fn non_finite_pixels_return_exact_summary_for_every_sample_type() {
        let mut pixels = [0.0, f32::NAN, 5.0, f32::INFINITY, f32::NEG_INFINITY];

        assert_eq!(
            validate_and_normalize(&mut pixels, 65_535.0, &CancelToken::never()).unwrap_err(),
            PixelValidationError::Nulls(NullSummary {
                count: 3,
                first_index: 1,
            })
        );
    }
}
