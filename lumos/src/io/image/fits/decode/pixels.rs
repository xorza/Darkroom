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
        let mut pixels = read_pixels(channel_ranges(plan, channel, row_start..row_end))
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
        normalize_and_validate(&mut pixels, plan.sample_divisor, &context.cancel).map_err(
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

/// Samples per parallel work item in the per-chunk passes below.
const CHUNK_SAMPLES: usize = 64 * 1024;

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

/// Divide a decode chunk into the pipeline's `[0, 1]` domain and reject the FITS nulls it carries,
/// in one pass over the samples.
///
/// The body is deliberately branch-free: the divide is unconditional and the finite test folds into
/// a boolean `|=`, so the loop vectorizes to `vdivps` plus a compare-and-or reduction. A
/// `if !finite { continue }` between the load and the divide costs far more than the second pass it
/// saves — it makes the divide scalar, and a scalar `f32` divide is several times the throughput of
/// the vector one.
///
/// Scaling before testing is both safe and slightly stronger: NaN and ±inf survive a division, so a
/// null is still a null afterwards, and a span that overflows a finite sample is caught here rather
/// than reaching the image.
///
/// Divides rather than multiplying by a precomputed reciprocal: the reciprocal of a span like 65535
/// is inexact in `f32`, and precision outranks throughput here. The `divisor == 1.0` test is hoisted
/// out of the loop rather than left in it — dividing by one is exact but not free, and it is the
/// common case for a floating-point HDU.
///
/// Locating the offending samples is a second scan that only a failing chunk pays for, by which
/// point the load is being abandoned anyway.
fn normalize_and_validate(
    pixels: &mut [f32],
    divisor: f32,
    cancel: &CancelToken,
) -> Result<(), PixelValidationError> {
    if cancel.is_cancelled() {
        return Err(PixelValidationError::Cancelled);
    }
    let scale = divisor != 1.0;
    let nulls = pixels
        .par_chunks_mut(CHUNK_SAMPLES)
        .enumerate()
        .map(|(chunk_index, chunk)| {
            if cancel.is_cancelled() {
                return Err(PixelValidationError::Cancelled);
            }
            let chunk_start = chunk_index * CHUNK_SAMPLES;
            let mut nonfinite = false;
            if scale {
                for pixel in chunk.iter_mut() {
                    *pixel /= divisor;
                    nonfinite |= !pixel.is_finite();
                }
            } else {
                for pixel in chunk.iter() {
                    nonfinite |= !pixel.is_finite();
                }
            }
            Ok(nonfinite.then(|| summarize_nulls(chunk, chunk_start)))
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

/// Count a failing chunk's nulls and locate the first, in the whole-plane index space `chunk_start`
/// anchors it to.
///
/// Off the hot path by construction: [`normalize_and_validate`] only reaches this once a chunk is
/// known to hold at least one null, which is a load that is about to fail.
fn summarize_nulls(chunk: &[f32], chunk_start: usize) -> NullSummary {
    let mut count = 0;
    let mut first_index = None;
    for (index, pixel) in chunk.iter().enumerate() {
        if !pixel.is_finite() {
            count += 1;
            first_index.get_or_insert(chunk_start + index);
        }
    }
    NullSummary {
        count,
        first_index: first_index.expect("only called for a chunk already known to hold a null"),
    }
}

#[cfg(test)]
mod tests {
    use common::CancelToken;

    use crate::io::image::fits::decode::pixels::{
        NullSummary, PixelValidationError, normalize_and_validate,
    };

    #[test]
    fn cancellation_stops_chunk_validation() {
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            normalize_and_validate(&mut [1.0, 2.0], 1.0, &cancel).unwrap_err(),
            PixelValidationError::Cancelled
        );
    }

    #[test]
    fn a_unit_divisor_accepts_every_finite_value_and_changes_none() {
        // The float-FITS path. Nothing is out of range to this pass — a negative calibration
        // residual and an undivided ADU value are both legitimate; only a null is not.
        let mut pixels = [-5.0, 0.0, 0.5, 2.0, 255.0, 65_535.0];
        let expected = pixels;
        normalize_and_validate(&mut pixels, 1.0, &CancelToken::never()).unwrap();
        assert_eq!(pixels, expected);
    }

    #[test]
    fn normalizing_maps_the_declared_span_onto_the_unit_interval() {
        // BITPIX = 16 with BZERO = 2¹⁵, BSCALE = 1: divisor |1| × (2¹⁶ − 1) = 65535, so the
        // unsigned span 0..=65535 lands exactly on [0, 1].
        let mut unsigned = [0.0f32, 16_384.0, 32_768.0, 65_535.0];
        normalize_and_validate(&mut unsigned, 65_535.0, &CancelToken::never()).unwrap();
        // The endpoints are exact; 16384/65535 = 0.2500038147, 32768/65535 = 0.5000076294.
        assert_eq!(unsigned[0], 0.0);
        assert!((unsigned[1] - 0.250_003_8).abs() < 1e-7, "{unsigned:?}");
        assert!((unsigned[2] - 0.500_007_6).abs() < 1e-7, "{unsigned:?}");
        assert_eq!(unsigned[3], 1.0);

        // The same divisor puts a signed frame on [-0.5, 0.5] around its own zero: the scale is
        // applied without an offset, so a negative sample stays negative.
        let mut signed = [-32_768.0f32, 0.0, 32_767.0];
        normalize_and_validate(&mut signed, 65_535.0, &CancelToken::never()).unwrap();
        assert!((signed[0] - -0.500_007_6).abs() < 1e-7, "{signed:?}");
        assert_eq!(signed[1], 0.0);
        assert!((signed[2] - 0.499_992_37).abs() < 1e-7, "{signed:?}");
    }

    #[test]
    fn a_span_that_overflows_a_finite_sample_is_reported_as_a_null() {
        // Scaling runs before the finite test, so a divisor small enough to push a finite sample
        // past f32::MAX is caught by the same pass instead of reaching the image.
        let mut pixels = [1.0e30f32, 0.0];
        assert_eq!(
            normalize_and_validate(&mut pixels, 1.0e-30, &CancelToken::never()).unwrap_err(),
            PixelValidationError::Nulls(NullSummary {
                count: 1,
                first_index: 0,
            })
        );
    }

    #[test]
    fn non_finite_pixels_return_exact_summary_for_every_sample_type() {
        // Nulls survive the divide, which is what lets the test run after it rather than before:
        // a NaN or ±inf divided by any span is still one, and is still counted here.
        let mut pixels = [0.0, f32::NAN, 5.0, f32::INFINITY, f32::NEG_INFINITY];

        assert_eq!(
            normalize_and_validate(&mut pixels, 65_535.0, &CancelToken::never()).unwrap_err(),
            PixelValidationError::Nulls(NullSummary {
                count: 3,
                first_index: 1,
            })
        );
    }
}
