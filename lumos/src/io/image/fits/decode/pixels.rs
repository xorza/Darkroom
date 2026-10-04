use std::fs::File;
use std::ops::Range;
use std::path::Path;

use arrayvec::ArrayVec;
use fits_well::header::Header;
use fits_well::image::Scaling;
use fits_well::io::StreamReader;
use rayon::prelude::*;

use common::CancelToken;

use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::CfaType;
use crate::io::image::cfa::QUANTIZATION_SIGMA_PER_STEP;
use crate::io::image::error::ImageError;
use crate::io::image::fits::decode::DecodedFitsImage;
use crate::io::image::fits::decode::plan::FitsDecodePlan;

use crate::io::image::fits::metadata::domain_keywords;
use crate::io::image::fits::metadata::{
    read_cfa_from_headers, read_metadata, read_row_order, read_text,
};
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
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags, SATURATION_FRACTION};
use crate::io::image::sample_domain::SampleDomain;
use crate::math::statistics::median_mut;
use crate::math::statistics::subsample::Subsample;

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
    // The keywords that can fail a load are read before a plane is: a frame they refuse costs no
    // decode.
    // A sensor frame is one plane. A cube's mosaic keywords describe the frame before someone
    // else's demosaic, so they say nothing about the planes decoded here.
    let cfa_type = if plan.dimensions.is_grayscale() {
        read_cfa_from_headers(
            header,
            plan.dimensions.height(),
            context.fits.unstated_bayer_pattern,
        )
        .map_err(|source| ImageError::fits(path, source))?
    } else {
        None
    };
    let row_order = read_row_order(header).map_err(|source| ImageError::fits(path, source))?;
    // The unit is half the sample domain, which decides whether frames combine. An all-blank BUNIT
    // parses to the single significant space §4.2.1.1 requires, which states no unit rather than an
    // empty one — left alone it would disagree with every real unit. Surrounding blanks are a
    // writer artifact rather than part of a unit name, so both ends go, and the domain comparison
    // downstream is then plain equality — see `SampleDomain::conversion_to` for why it stops there
    // and does not fold case.
    let unit = read_text(header, "BUNIT")
        .map_err(|source| ImageError::fits(path, source))?
        .map(|unit| unit.trim().to_owned())
        .filter(|unit| !unit.is_empty());
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
    let scan = planes
        .iter()
        .map(|plane| plane.scan)
        .fold(SampleScan::EMPTY, SampleScan::merge);
    if !plan.sample_scale.accepts_maximum(scan.maximum) {
        return Err(ImageError::fits_unsupported(
            path,
            format!(
                "floating-point samples reach {}, but nothing declares their scale; \
                 give FitsFloatScale::FullScale, or a DATAMAX",
                scan.maximum
            ),
        ));
    }
    let header_err = |source| ImageError::fits(path, source);
    let quantization_sigma =
        match domain_keywords::read_quantization_sigma(header).map_err(header_err)? {
            Some(sigma) => Some(sigma),
            None => plan.sample_type.is_integer().then(|| {
                let step =
                    scan.integer_step(plan.scaling.bscale) / f64::from(plan.sample_scale.divisor);
                step as f32 * QUANTIZATION_SIGMA_PER_STEP
            }),
        };
    let pedestal = domain_keywords::read_pedestal(header)
        .map_err(header_err)?
        .unwrap_or(context.fits.pedestal);
    let mut metadata = read_metadata(header, plan.shape, plan.sample_type);
    // DATAMAX is a saturation level in the file's sample units, so it only stays comparable to the
    // samples if it is divided by the same span they were.
    if let Some(data_max) = &mut metadata.data_max {
        *data_max /= f64::from(plan.sample_scale.divisor);
    }
    let saturation = metadata
        .data_max
        .map(|data_max| SATURATION_FRACTION * data_max as f32);
    metadata.saturation_flagged = saturation.is_some();
    let flags = resolve_flags(
        path,
        &mut planes,
        plan.dimensions,
        context.fits.nulls,
        saturation,
    )?;
    let pixels = LinearPixels::from_planar_channels(
        plan.dimensions,
        planes.into_iter().map(|plane| plane.samples),
    );
    metadata.domain = Some(SampleDomain {
        scale: plan.sample_scale.physical,
        origin: plan.sample_scale.origin,
        pedestal,
        unit,
    });
    metadata.quantization_sigma = quantization_sigma;
    metadata.provenance = Some(ImageProvenance {
        container: SourceContainer::Fits,
        decoder: DecoderProvenance::FitsWell,
        transfer: TransferProvenance::FitsNormalized(FitsTransferProvenance {
            bscale: plan.scaling.bscale,
            bzero: plan.scaling.bzero,
            hdu,
            checksum,
        }),
        color: if cfa_type.is_some_and(|cfa_type| cfa_type != CfaType::Mono) {
            ColorProvenance::SensorCfa
        } else if plan.dimensions.is_grayscale() {
            ColorProvenance::Monochrome
        } else {
            ColorProvenance::Unspecified
        },
        clipped: false,
        demosaic: DemosaicProvenance::None,
        // Recorded, not acted on: the rows above were copied in file order whatever this says, and
        // only the Bayer phase was corrected for it. What it buys is that a set mixing the two
        // orders — which loads as mutually mirrored images — can be named as such.
        row_order,
    });

    Ok(DecodedFitsImage {
        metadata,
        cfa_type,
        pixels,
        flags,
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
    scan: SampleScan,
}

/// Apply the caller's null policy to a decoded image's planes — reject the load, or fill the nulls
/// — and hand back the flags the samples settle: [`QualityFlags::NO_DATA`] where they were null, and
/// [`QualityFlags::SATURATED`] where a channel reaches `saturation`, a level in the decoded domain.
///
/// A frame with no nulls and no saturation level — most frames without a `DATAMAX` — returns
/// before any scan, so the feature costs one sum over at most three integers.
fn resolve_flags(
    path: &Path,
    planes: &mut [DecodedPlane],
    dimensions: ImageDimensions,
    policy: FitsNullPolicy,
    saturation: Option<f32>,
) -> Result<Option<PixelFlags>, ImageError> {
    let count: usize = planes
        .iter()
        .filter_map(|plane| plane.nulls)
        .map(|nulls| nulls.count)
        .sum();
    if count == 0 && saturation.is_none() {
        return Ok(None);
    }

    if count > 0 && policy == FitsNullPolicy::Reject {
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
        return Err(ImageError::fits_unsupported(
            path,
            format!(
                "image contains {count} null/non-finite samples; first at linear index {first_index}"
            ),
        ));
    }

    // Before the fill, which is what erases the evidence. A non-finite sample fails every
    // comparison, so a null is never also saturated.
    let flags = {
        let samples = planes
            .iter()
            .map(|plane| plane.samples.as_slice())
            .collect::<ArrayVec<&[f32], 3>>();
        PixelFlags::from_fn(dimensions.size(), |index| {
            if samples.iter().any(|plane| !plane[index].is_finite()) {
                QualityFlags::NO_DATA
            } else if saturation
                .is_some_and(|level| samples.iter().any(|plane| plane[index] >= level))
            {
                QualityFlags::SATURATED
            } else {
                QualityFlags::default()
            }
        })
    };
    if count == 0 {
        return Ok(flags);
    }
    for plane in planes.iter_mut() {
        if let Some(nulls) = plane.nulls {
            fill_nulls(&mut plane.samples, nulls.count);
        }
    }
    let flags = flags.expect("a nonzero count means at least one plane holds a non-finite sample");
    // Only for a frame that has them, and the samples the caller is about to read are partly fill
    // with nothing in the frame itself to say so.
    tracing::info!(
        pixels = flags.count(QualityFlags::NO_DATA),
        of = dimensions.pixel_count(),
        "FITS image declares pixels with no measurement"
    );
    Ok(Some(flags))
}

/// Samples the fill's median is taken over, at most.
///
/// The median of a hundred thousand values drawn evenly across a plane and the median of all
/// twenty-four million agree to far more precision than a stand-in for missing data needs, and the
/// bound is what keeps this off the memory preflight: the scratch is a fixed 400 KB rather than a
/// second copy of the plane the preflight above did not budget for. `defect_map::sampling` bounds
/// its own medians the same way and for the same reason.
const FILL_MEDIAN_SAMPLES: usize = 100_000;

/// Replace a plane's non-finite samples with the median of its finite ones.
///
/// What sits under a null is not data and [`NullMask`] says so, but the stages that measure a whole
/// plane mostly do not consult the mask, so this value is what they see. The median is the frame's
/// own background level, which leaves a masked region a flat patch instead of the hard-edged hole a
/// zero fill would cut — and a hard edge is what manufactures star detections and drags a
/// background estimate. A deliberate stand-in, not a correction.
fn fill_nulls(samples: &mut [f32], null_count: usize) {
    debug_assert!(null_count > 0 && null_count <= samples.len());
    // Every `stride`-th *finite* sample rather than every `stride`-th sample: nulls arrive in
    // regions — a mosaic edge, a coverage gap — so striding the plane itself would draw its whole
    // quota from one side of a frame that is masked down the other.
    let sample = Subsample::new(samples.len() - null_count, FILL_MEDIAN_SAMPLES);
    let mut finite = Vec::with_capacity(sample.count());
    finite.extend(
        samples
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .step_by(sample.stride()),
    );
    // A wholly-null plane has no level of its own to borrow, and no guess is better than any
    // other. The mask says every pixel of it is missing, which is the part that has to survive.
    let fill = if finite.is_empty() {
        0.0
    } else {
        median_mut(&mut finite)
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
    let mut scan = SampleScan::EMPTY;
    let integer = plan.sample_type.is_integer().then_some(plan.scaling);
    for row_start in (0..height).step_by(plan.rows_per_chunk) {
        context.check_cancelled(path)?;
        let row_end = row_start.saturating_add(plan.rows_per_chunk).min(height);
        let expected_chunk = (row_end - row_start) * width;
        let mut pixels = read_pixels(channel_ranges(plan, channel, row_start..row_end))
            .map_err(|source| ImageError::fits(path, source))?;
        context.check_cancelled(path)?;
        if pixels.len() != expected_chunk {
            return Err(ImageError::fits_unsupported(
                path,
                format!(
                    "channel {channel} rows {row_start}..{row_end} contain {} pixels; expected {expected_chunk}",
                    pixels.len()
                ),
            ));
        }
        let ChunkScan {
            nulls: chunk_nulls,
            scan: chunk_scan,
        } = normalize_and_scan(
            &mut pixels,
            plan.sample_scale.divisor,
            integer,
            &context.cancel,
        )
        .map_err(|Cancelled| ImageError::cancelled(path))?;
        scan = scan.merge(chunk_scan);
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
        scan,
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
    const fn offset_by(self, by: usize) -> Self {
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

/// What the normalize pass learns about the samples beside their nulls.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SampleScan {
    /// The OR of every stored integer, for an integer `BITPIX`: its trailing zeros are the ones
    /// every sample shares.
    raw_bits: u64,
    /// Whether every stored integer was below `2²⁴`, where the `f32` the reader hands back holds
    /// it exactly. Past that the bits above describe the conversion, not the data.
    raw_exact: bool,
    /// The largest finite sample after the division.
    maximum: f32,
}

/// A chunk's nulls and its [`SampleScan`].
#[derive(Debug, Clone, Copy, PartialEq)]
struct ChunkScan {
    nulls: Option<NullSummary>,
    scan: SampleScan,
}

/// Integers an `f32` holds exactly: every magnitude below `2²⁴`.
const EXACT_F32_INTEGER: f64 = 16_777_216.0;

impl SampleScan {
    const EMPTY: Self = Self {
        raw_bits: 0,
        raw_exact: true,
        maximum: f32::NEG_INFINITY,
    };

    const fn merge(self, other: Self) -> Self {
        Self {
            raw_bits: self.raw_bits | other.raw_bits,
            raw_exact: self.raw_exact && other.raw_exact,
            maximum: self.maximum.max(other.maximum),
        }
    }

    /// One quantization step in physical units: `|BSCALE|·2^z`, where `z` is the trailing zeros
    /// every stored integer shares. 12- or 14-bit data written left-justified into a 16-bit
    /// container steps by 16 or 4 stored units, not by one. The container's own step stands when
    /// nothing can be read off the samples: all of them zero, or some too large to have reached
    /// this pass exactly.
    fn integer_step(&self, bscale: f64) -> f64 {
        let shared_zeros = if self.raw_exact && self.raw_bits != 0 {
            self.raw_bits.trailing_zeros()
        } else {
            0
        };
        bscale.abs() * 2.0f64.powi(shared_zeros as i32)
    }
}

/// Divide a decode chunk into the pipeline's `[0, 1]` domain, locate the FITS nulls it carries, and
/// scan it for its [`SampleScan`], in one pass over the samples.
///
/// The divide loop is deliberately branch-free: the divide is unconditional, and the finite test
/// and the maximum fold into a boolean `|=` and a select, so the loop vectorizes to `vdivps` plus
/// compare-and-reduce. A `if !finite { continue }` between the load and the divide costs far more
/// than the second pass it saves — it makes the divide scalar, and a scalar `f32` divide is several
/// times the throughput of the vector one. The stored-integer OR is its own loop over the chunk,
/// still in cache, and only an integer `BITPIX` pays for it.
///
/// Scaling before testing is both safe and slightly stronger: NaN and ±inf survive a division, so a
/// null is still a null afterwards, and a span that overflows a finite sample is caught here rather
/// than reaching the image.
///
/// Divides rather than multiplying by a precomputed reciprocal: the reciprocal of a span like 65535
/// is inexact in `f32`, and precision outranks throughput here. The `divisor == 1.0` test is
/// hoisted out of the loop rather than left in it — dividing by one is exact but not free, and it
/// is the common case for a floating-point HDU.
///
/// Locating the offending samples is a second scan that only a chunk holding one pays for, so a
/// frame with no nulls — every frame from a sensor — never runs it at all.
fn normalize_and_scan(
    pixels: &mut [f32],
    divisor: f32,
    integer: Option<Scaling>,
    cancel: &CancelToken,
) -> Result<ChunkScan, Cancelled> {
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
            let mut scan = SampleScan::EMPTY;
            if let Some(integer) = integer {
                for &pixel in chunk.iter().filter(|pixel| pixel.is_finite()) {
                    let stored = ((f64::from(pixel) - integer.bzero) / integer.bscale).round();
                    scan.raw_exact &= stored.abs() < EXACT_F32_INTEGER;
                    // Two's complement keeps a negative value's trailing zeros.
                    scan.raw_bits |= (stored as i64).cast_unsigned();
                }
            }
            let mut nonfinite = false;
            let mut maximum = f32::NEG_INFINITY;
            if scale {
                for pixel in chunk.iter_mut() {
                    *pixel /= divisor;
                    let finite = pixel.is_finite();
                    nonfinite |= !finite;
                    maximum = maximum.max(if finite { *pixel } else { f32::NEG_INFINITY });
                }
            } else {
                for &pixel in chunk.iter() {
                    let finite = pixel.is_finite();
                    nonfinite |= !finite;
                    maximum = maximum.max(if finite { pixel } else { f32::NEG_INFINITY });
                }
            }
            scan.maximum = maximum;
            Ok(ChunkScan {
                nulls: nonfinite.then(|| summarize_nulls(chunk, chunk_start)),
                scan,
            })
        })
        .try_reduce(
            || ChunkScan {
                nulls: None,
                scan: SampleScan::EMPTY,
            },
            |left, right| {
                Ok(ChunkScan {
                    nulls: match (left.nulls, right.nulls) {
                        (None, value) | (value, None) => value,
                        (Some(left), Some(right)) => Some(left.merge(right)),
                    },
                    scan: left.scan.merge(right.scan),
                })
            },
        )
}

/// Count a chunk's nulls and locate the first, in the whole-span index space `chunk_start` anchors
/// it to.
///
/// Off the hot path by construction: [`normalize_and_scan`] only reaches this once a chunk
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

    use crate::io::cancelled::Cancelled;
    use crate::io::image::fits::decode::pixels::{
        FILL_MEDIAN_SAMPLES, NullSummary, SampleScan, fill_nulls, normalize_and_scan,
    };
    use fits_well::image::Scaling;

    #[test]
    fn cancellation_stops_chunk_validation() {
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            normalize_and_scan(&mut [1.0, 2.0], 1.0, None, &cancel).unwrap_err(),
            Cancelled
        );
    }

    #[test]
    fn a_unit_divisor_accepts_every_finite_value_and_changes_none() {
        // The float-FITS path. Nothing is out of range to this pass — a negative calibration
        // residual and an undivided ADU value are both legitimate.
        let mut pixels = [-5.0, 0.0, 0.5, 2.0, 255.0, 65_535.0];
        let expected = pixels;
        let scan = normalize_and_scan(&mut pixels, 1.0, None, &CancelToken::never()).unwrap();
        assert_eq!(scan.nulls, None);
        assert_eq!(scan.scan.maximum, 65_535.0);
        assert_eq!(pixels, expected);
    }

    #[test]
    fn normalizing_maps_the_declared_span_onto_the_unit_interval() {
        // BITPIX = 16 with BZERO = 2¹⁵, BSCALE = 1: divisor |1| × (2¹⁶ − 1) = 65535, so the
        // unsigned span 0..=65535 lands exactly on [0, 1].
        let mut unsigned = [0.0f32, 16_384.0, 32_768.0, 65_535.0];
        let unsigned_convention = Scaling {
            bscale: 1.0,
            bzero: 32_768.0,
            blank: None,
        };
        let scan = normalize_and_scan(
            &mut unsigned,
            65_535.0,
            Some(unsigned_convention),
            &CancelToken::never(),
        )
        .unwrap()
        .scan;
        // The stored integers are v − 32768: −32768, −16384, 0 and 32767. The last is odd, so they
        // share no trailing zero and the step is one stored unit.
        assert_eq!(scan.raw_bits.trailing_zeros(), 0);
        assert!(scan.raw_exact);
        assert_eq!(scan.integer_step(1.0), 1.0);
        assert_eq!(scan.maximum, 1.0);
        // The endpoints are exact; 16384/65535 = 0.2500038147, 32768/65535 = 0.5000076294.
        assert_eq!(unsigned[0], 0.0);
        assert!((unsigned[1] - 0.250_003_8).abs() < 1e-7, "{unsigned:?}");
        assert!((unsigned[2] - 0.500_007_6).abs() < 1e-7, "{unsigned:?}");
        assert_eq!(unsigned[3], 1.0);

        // The same divisor puts a signed frame on [-0.5, 0.5] around its own zero: the scale is
        // applied without an offset, so a negative sample stays negative.
        let mut signed = [-32_768.0f32, 0.0, 32_767.0];
        normalize_and_scan(&mut signed, 65_535.0, None, &CancelToken::never()).unwrap();
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
            normalize_and_scan(&mut pixels, 1.0e-30, None, &CancelToken::never())
                .unwrap()
                .nulls,
            Some(NullSummary {
                count: 1,
                first_index: 0,
            })
        );
    }

    #[test]
    fn nan_and_both_infinities_are_counted_after_the_divide() {
        // Nulls survive the divide, which is what lets the test run after it rather than before:
        // a NaN or ±inf divided by any span is still one, and is still counted here.
        let mut pixels = [0.0, f32::NAN, 5.0, f32::INFINITY, f32::NEG_INFINITY];

        let scan = normalize_and_scan(&mut pixels, 65_535.0, None, &CancelToken::never()).unwrap();
        assert_eq!(
            scan.nulls,
            Some(NullSummary {
                count: 3,
                first_index: 1,
            })
        );
        // The maximum is over the finite samples only: 5/65535, not infinity.
        assert_eq!(scan.scan.maximum, 5.0 / 65_535.0);
    }

    /// The shared trailing zeros of the stored integers give the step: stored values that are all
    /// multiples of 16 step by 16 × |BSCALE|. A value past 2²⁴ is not exact in the f32 the reader
    /// hands back, and then nothing is read off the bits.
    #[test]
    fn the_integer_step_is_the_shared_trailing_zeros() {
        let shifted = SampleScan {
            raw_bits: 0b1_0000 | 0b11_0000 | 0b1111_1111_0000,
            raw_exact: true,
            maximum: 1.0,
        };
        assert_eq!(shifted.integer_step(0.5), 8.0);
        assert_eq!(
            SampleScan {
                raw_exact: false,
                ..shifted
            }
            .integer_step(0.5),
            0.5
        );
        assert_eq!(SampleScan::EMPTY.integer_step(2.0), 2.0);
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

    #[test]
    fn the_fills_median_is_sampled_rather_than_taken_over_a_whole_plane() {
        // Twice the sample bound of valid data, so the stride is exactly 2 and the scratch holds
        // `FILL_MEDIAN_SAMPLES` values instead of a second copy of the plane — the allocation the
        // decode's memory preflight does not budget for.
        //
        // Valid samples are the ramp 0..200_000. Every second one is 0, 2, … 199_998, whose median
        // is the mean of its two middle values (99_998 and 100_000) = 99_999. The whole ramp's
        // median is 99_999.5, so the bound costs half a unit in two hundred thousand.
        const VALID: usize = 2 * FILL_MEDIAN_SAMPLES;
        let mut samples: Vec<f32> = (0..VALID).map(|index| index as f32).collect();
        samples.push(f32::NAN);
        fill_nulls(&mut samples, 1);
        assert_eq!(samples[VALID], 99_999.0);

        // The stride runs over the finite samples, not over plane positions: masking the whole
        // first half would otherwise spend that half of the quota on pixels that are dropped
        // anyway, and draw the median from the tail alone.
        let mut lopsided: Vec<f32> = (0..VALID)
            .map(|index| {
                if index < VALID / 2 {
                    f32::NAN
                } else {
                    index as f32
                }
            })
            .collect();
        fill_nulls(&mut lopsided, VALID / 2);
        // The surviving ramp is 100_000..200_000, whose median is 149_999.5; sampling every one of
        // them (stride 1, since the survivors are exactly the bound) reproduces it exactly.
        assert_eq!(lopsided[0], 149_999.5);
    }
}
