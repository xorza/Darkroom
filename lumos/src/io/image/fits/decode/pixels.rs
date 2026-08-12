use std::fs::File;
use std::ops::Range;
use std::path::Path;

use arrayvec::ArrayVec;
use fits_well::header::Header;
use fits_well::io::StreamReader;
use rayon::prelude::*;

use common::CancelToken;

use crate::io::image::error::ImageError;
use crate::io::image::fits::decode::DecodedFitsImage;
use crate::io::image::fits::decode::plan::FitsDecodePlan;
use crate::io::image::fits::error::{fits_err, fits_unsupported};
use crate::io::image::fits::metadata::{read_metadata, read_text};
use crate::io::image::fits::options::FitsNullPolicy;
use crate::io::image::fits::provenance::{
    FitsChecksumProvenance, FitsHduProvenance, FitsTransferProvenance,
};
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, SourceContainer,
    TransferProvenance,
};
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::load_context::LoadContext;
use crate::io::image::null_mask::NullMask;
use crate::math::statistics::median_f32_mut;

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
    let channel_count = if plan.dimensions.is_rgb() { 3 } else { 1 };
    let mut planes = ArrayVec::<DecodedPlane, 3>::new();
    for channel in 0..channel_count {
        planes.push(read_fits_plane(
            path,
            &plan,
            channel,
            context,
            &mut read_pixels,
        )?);
    }
    let nulls = resolve_nulls(path, &mut planes, plan.dimensions, context.fits.nulls)?;
    let pixels = LinearPixels::from_planar_channels(
        plan.dimensions,
        planes.into_iter().map(|plane| plane.samples),
    );

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
            // An all-blank BUNIT parses to the single significant space §4.2.1.1 requires, which
            // states no unit rather than an empty one — left alone it would disagree with every
            // real unit. Surrounding blanks are a writer artifact rather than part of a unit name,
            // so both ends go, and the domain comparison downstream is then plain equality — see
            // `SampleDomain::commensurate_with` for why it stops there and does not fold case.
            unit: read_text(header, "BUNIT")
                .map_err(|source| fits_err(path, source))?
                .map(|unit| unit.trim().to_owned())
                .filter(|unit| !unit.is_empty()),
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

    Ok(DecodedFitsImage {
        metadata,
        pixels,
        nulls,
    })
}

/// One decoded channel plane, and where its nulls are.
///
/// The count and first index come out of the pass that scaled the samples, which already walked
/// every one of them; locating them a second time to decide policy would be a second full scan.
#[derive(Debug)]
struct DecodedPlane {
    samples: Vec<f32>,
    nulls: Option<NullSummary>,
}

/// Apply the caller's null policy to a decoded image's planes: reject the load, or fill the nulls
/// and hand back the mask that says where they were.
///
/// A frame with no nulls — every frame from a sensor, and most from a survey — returns before
/// either branch, so the whole feature costs one sum over at most three integers.
fn resolve_nulls(
    path: &Path,
    planes: &mut [DecodedPlane],
    dimensions: ImageDimensions,
    policy: FitsNullPolicy,
) -> Result<Option<NullMask>, ImageError> {
    let count: usize = planes
        .iter()
        .filter_map(|plane| plane.nulls)
        .map(|nulls| nulls.count)
        .sum();
    if count == 0 {
        return Ok(None);
    }

    if policy == FitsNullPolicy::Reject {
        let first_index = planes
            .iter()
            .enumerate()
            .find_map(|(channel, plane)| {
                plane
                    .nulls
                    .map(|nulls| channel * dimensions.pixel_count() + nulls.first_index)
            })
            .expect("a nonzero count means at least one plane reported a null");
        // Samples, not pixels: this counts each channel's nulls separately, and the index is in the
        // channel-major sample space the count belongs to. The pixel figure needs the mask, which
        // this branch does not build.
        return Err(fits_unsupported(
            path,
            format!(
                "image contains {count} null/non-finite samples; first at linear index {first_index}"
            ),
        ));
    }

    // Before the fill, which is what erases the evidence.
    let mask = {
        let samples = planes
            .iter()
            .map(|plane| plane.samples.as_slice())
            .collect::<ArrayVec<&[f32], 3>>();
        NullMask::of_non_finite(dimensions.size(), &samples)
            .expect("a nonzero count means at least one plane holds a non-finite sample")
    };
    for plane in planes.iter_mut() {
        if let Some(nulls) = plane.nulls {
            fill_nulls(&mut plane.samples, nulls.count);
        }
    }
    // Only for a frame that has them, and the samples the caller is about to read are partly fill
    // with nothing in the frame itself to say so.
    tracing::info!(
        pixels = mask.count(),
        of = dimensions.pixel_count(),
        "FITS image declares pixels with no measurement"
    );
    Ok(Some(mask))
}

/// Replace a plane's non-finite samples with the median of its finite ones.
///
/// What sits under a null is not data and [`NullMask`] says so, but no stage consults the mask yet,
/// so this value is what they all measure. The median is the frame's own background level, which
/// leaves a masked region a flat patch instead of the hard-edged hole a zero fill would cut — and a
/// hard edge is what manufactures star detections and drags a background estimate. A deliberate
/// stopgap for the stages that have not been taught the mask, not a correction.
fn fill_nulls(samples: &mut [f32], null_count: usize) {
    debug_assert!(null_count > 0 && null_count <= samples.len());
    // Exact: the decode pass counted the nulls, so what is left is what survives the filter.
    let mut finite = Vec::with_capacity(samples.len() - null_count);
    finite.extend(samples.iter().copied().filter(|value| value.is_finite()));
    // A wholly-null plane has no level of its own to borrow, and no guess is better than any
    // other. The mask says every pixel of it is missing, which is the part that has to survive.
    let fill = if finite.is_empty() {
        0.0
    } else {
        median_f32_mut(&mut finite)
    };
    for value in samples.iter_mut().filter(|value| !value.is_finite()) {
        *value = fill;
    }
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
) -> Result<DecodedPlane, ImageError> {
    let width = plan.dimensions.width();
    let height = plan.dimensions.height();
    let expected_pixels = plan.dimensions.pixel_count();
    let mut output = vec![0.0; expected_pixels];
    let mut nulls: Option<NullSummary> = None;
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
        let chunk_nulls =
            normalize_and_locate_nulls(&mut pixels, plan.sample_divisor, &context.cancel).map_err(
                |Cancelled| ImageError::Cancelled {
                    path: path.to_path_buf(),
                },
            )?;
        // Each chunk locates its nulls in its own index space; the plane's is what a caller can act
        // on, so the offset is applied here rather than threaded into the pass.
        if let Some(chunk_nulls) = chunk_nulls {
            let chunk_nulls = chunk_nulls.offset_by(row_start * width);
            nulls = Some(match nulls {
                None => chunk_nulls,
                Some(nulls) => nulls.merge(chunk_nulls),
            });
        }
        let start = row_start * width;
        output[start..start + expected_chunk].copy_from_slice(&pixels);
    }
    Ok(DecodedPlane {
        samples: output,
        nulls,
    })
}

/// Samples per parallel work item in the per-chunk passes below.
const CHUNK_SAMPLES: usize = 64 * 1024;

/// How many nulls a span of samples holds and where the first one is, in that span's own index
/// space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NullSummary {
    count: usize,
    first_index: usize,
}

impl NullSummary {
    /// Restate this summary in an index space `by` samples earlier — a chunk's, in its plane's.
    fn offset_by(self, by: usize) -> Self {
        Self {
            count: self.count,
            first_index: self.first_index + by,
        }
    }

    /// Fold in another span's summary. Both must already be in the same index space.
    fn merge(self, other: Self) -> Self {
        Self {
            count: self.count + other.count,
            first_index: self.first_index.min(other.first_index),
        }
    }
}

/// The only way the pass below fails, now that a null is data rather than an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cancelled;

/// Divide a decode chunk into the pipeline's `[0, 1]` domain and locate the FITS nulls it carries,
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
/// Locating the offending samples is a second scan that only a chunk holding one pays for, so a
/// frame with no nulls — every frame from a sensor — never runs it at all.
fn normalize_and_locate_nulls(
    pixels: &mut [f32],
    divisor: f32,
    cancel: &CancelToken,
) -> Result<Option<NullSummary>, Cancelled> {
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    let scale = divisor != 1.0;
    pixels
        .par_chunks_mut(CHUNK_SAMPLES)
        .enumerate()
        .map(|(chunk_index, chunk)| {
            if cancel.is_cancelled() {
                return Err(Cancelled);
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
                    (Some(left), Some(right)) => Some(left.merge(right)),
                })
            },
        )
}

/// Count a chunk's nulls and locate the first, in the whole-span index space `chunk_start` anchors
/// it to.
///
/// Off the hot path by construction: [`normalize_and_locate_nulls`] only reaches this once a chunk
/// is known to hold at least one null.
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
        Cancelled, NullSummary, fill_nulls, normalize_and_locate_nulls,
    };

    #[test]
    fn cancellation_stops_chunk_validation() {
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            normalize_and_locate_nulls(&mut [1.0, 2.0], 1.0, &cancel).unwrap_err(),
            Cancelled
        );
    }

    #[test]
    fn a_unit_divisor_accepts_every_finite_value_and_changes_none() {
        // The float-FITS path. Nothing is out of range to this pass — a negative calibration
        // residual and an undivided ADU value are both legitimate.
        let mut pixels = [-5.0, 0.0, 0.5, 2.0, 255.0, 65_535.0];
        let expected = pixels;
        assert_eq!(
            normalize_and_locate_nulls(&mut pixels, 1.0, &CancelToken::never()).unwrap(),
            None
        );
        assert_eq!(pixels, expected);
    }

    #[test]
    fn normalizing_maps_the_declared_span_onto_the_unit_interval() {
        // BITPIX = 16 with BZERO = 2¹⁵, BSCALE = 1: divisor |1| × (2¹⁶ − 1) = 65535, so the
        // unsigned span 0..=65535 lands exactly on [0, 1].
        let mut unsigned = [0.0f32, 16_384.0, 32_768.0, 65_535.0];
        normalize_and_locate_nulls(&mut unsigned, 65_535.0, &CancelToken::never()).unwrap();
        // The endpoints are exact; 16384/65535 = 0.2500038147, 32768/65535 = 0.5000076294.
        assert_eq!(unsigned[0], 0.0);
        assert!((unsigned[1] - 0.250_003_8).abs() < 1e-7, "{unsigned:?}");
        assert!((unsigned[2] - 0.500_007_6).abs() < 1e-7, "{unsigned:?}");
        assert_eq!(unsigned[3], 1.0);

        // The same divisor puts a signed frame on [-0.5, 0.5] around its own zero: the scale is
        // applied without an offset, so a negative sample stays negative.
        let mut signed = [-32_768.0f32, 0.0, 32_767.0];
        normalize_and_locate_nulls(&mut signed, 65_535.0, &CancelToken::never()).unwrap();
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
            normalize_and_locate_nulls(&mut pixels, 1.0e-30, &CancelToken::never()).unwrap(),
            Some(NullSummary {
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
            normalize_and_locate_nulls(&mut pixels, 65_535.0, &CancelToken::never()).unwrap(),
            Some(NullSummary {
                count: 3,
                first_index: 1,
            })
        );
    }

    #[test]
    fn summaries_from_two_chunks_combine_into_the_planes_index_space() {
        // Row-chunk 0 holds one null at 2; row-chunk 1 starts 8 samples in and holds two, at its
        // own 0 and 3 — pixels 8 and 11 of the plane. Merged: three nulls, first at 2.
        let first = NullSummary {
            count: 1,
            first_index: 2,
        };
        let second = NullSummary {
            count: 2,
            first_index: 0,
        };
        assert_eq!(
            first.merge(second.offset_by(8)),
            NullSummary {
                count: 3,
                first_index: 2,
            }
        );
        // The offset moves only the location, and the merge takes the earlier one whichever side
        // it arrives on.
        assert_eq!(second.offset_by(8).first_index, 8);
        assert_eq!(second.offset_by(8).merge(first).first_index, 2);
    }

    #[test]
    fn nulls_are_filled_with_the_median_of_the_finite_samples() {
        // Finite samples 1, 2, 3, 4, 10 — five of them, so the median is the middle one, 3. Both
        // nulls take it, and no finite sample moves.
        let mut samples = [1.0, f32::NAN, 2.0, 3.0, f32::INFINITY, 4.0, 10.0];
        fill_nulls(&mut samples, 2);
        assert_eq!(samples, [1.0, 3.0, 2.0, 3.0, 3.0, 4.0, 10.0]);

        // Zero fills a plane with no finite sample to borrow a level from: nothing else is more
        // right, and the mask is what records that none of it is data.
        let mut empty = [f32::NAN; 3];
        fill_nulls(&mut empty, 3);
        assert_eq!(empty, [0.0; 3]);
    }
}
