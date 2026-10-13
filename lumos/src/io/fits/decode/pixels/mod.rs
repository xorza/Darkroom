use std::fs::File;
use std::ops::Range;
use std::path::Path;

use arrayvec::ArrayVec;
use fits_well::header::Header;
use fits_well::image::{Scaling, View as FitsView};
use fits_well::io::StreamReader;
use rayon::prelude::*;

use common::CancelToken;

use crate::concurrency;
use crate::error::Cancelled;
use crate::image::cfa::QUANTIZATION_SIGMA_PER_STEP;
use crate::image::cfa::cfa_type::CfaType;
use crate::io::error::{FitsRefusal, ImageError};
use crate::io::fits::decode::DecodedFitsImage;
use crate::io::fits::decode::hdu_sections::HduSections;
use crate::io::fits::decode::plan::FitsDecodePlan;
use crate::io::fits::decode::selection;

use crate::image::image_provenance::{
    ColorProvenance, DecoderProvenance, DemosaicProvenance, ImageProvenance, SourceContainer,
    TransferProvenance,
};
use crate::image::linear_pixels::LinearPixels;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::{QualityFlags, SATURATION_FRACTION};
use crate::image::sample_domain::SampleDomain;
use crate::io::fits::fits_checksum_provenance::FitsChecksumProvenance;
use crate::io::fits::fits_load_options::{FitsChecksumPolicy, FitsNullPolicy};
use crate::io::fits::fits_transfer_provenance::{FitsHduProvenance, FitsTransferProvenance};
use crate::io::fits::metadata;
use crate::io::fits::metadata::domain_keywords;
use crate::io::load_context::LoadContext;
use crate::math::statistics::median_mut;
use crate::math::statistics::subsample::Subsample;

/// The decode of one image HDU: the HDU, its header and the plan its preflight made, and the file
/// it is read from under the load's context.
#[derive(Debug)]
pub(super) struct HduDecode<'a> {
    pub(super) hdu: FitsHduProvenance,
    pub(super) header: &'a Header,
    pub(super) plan: &'a FitsDecodePlan,
    pub(super) path: &'a Path,
    pub(super) context: &'a LoadContext,
}

impl HduDecode<'_> {
    /// Decode the HDU from `reader`, its checksum checked under `policy` from the bytes the decode
    /// reads, and judged before anything is returned: a refused checksum fails the load as before
    /// the decode it would, at the cost of the decode.
    pub(super) fn read_stream(
        self,
        reader: &mut StreamReader<File>,
        policy: FitsChecksumPolicy,
    ) -> Result<DecodedFitsImage, ImageError> {
        let index = self.hdu.index;
        let path = self.path;
        let context = self.context;
        if policy == FitsChecksumPolicy::Ignore {
            return self.read(&mut HduSections::new(reader, index), |_| {
                Ok(FitsChecksumProvenance::NOT_CHECKED)
            });
        }
        let mut sections =
            HduSections::summed(reader, index).map_err(|source| ImageError::fits(path, source))?;
        self.read(&mut sections, |sections| {
            context.check_cancelled(path)?;
            let report = sections
                .finish_checksum()
                .map_err(|source| ImageError::fits(path, source))?;
            selection::judge_checksum(report, index, path, policy)
        })
    }

    /// Decode the HDU through `sections`, and take its checksum provenance from `checksum` once
    /// the planes are read.
    pub(super) fn read(
        self,
        sections: &mut HduSections<'_>,
        checksum: impl FnOnce(&mut HduSections<'_>) -> Result<FitsChecksumProvenance, ImageError>,
    ) -> Result<DecodedFitsImage, ImageError> {
        let Self {
            header,
            plan,
            path,
            context,
            ..
        } = self;
        tracing::debug!(
            source_bytes = plan.source_bytes,
            decoded_bytes = plan.decoded_bytes,
            peak_bytes = plan.peak_bytes,
            "FITS image passed header-first memory preflight"
        );
        // The keywords that can fail a load are read before a plane is: a frame they refuse costs
        // no decode. A sensor frame is one plane. A cube's mosaic keywords describe the frame
        // before someone else's demosaic, so they say nothing about the planes decoded here.
        let cfa_type = if plan.dimensions.is_grayscale() {
            metadata::read_cfa(
                header,
                plan.dimensions.height(),
                context.fits.unstated_bayer_pattern,
            )
            .map_err(|source| ImageError::fits(path, source))?
        } else {
            None
        };
        let row_order =
            metadata::read_row_order(header).map_err(|source| ImageError::fits(path, source))?;
        // The unit is half the sample domain, which decides whether frames combine. An all-blank
        // BUNIT parses to the single significant space §4.2.1.1 requires, which states no unit
        // rather than an empty one — left alone it would disagree with every real unit. Surrounding
        // blanks are a writer artifact rather than part of a unit name, so both ends go, and the
        // domain comparison downstream is then plain equality — see `SampleDomain::conversion_to`
        // for why it stops there and does not fold case.
        let unit = metadata::read_text(header, "BUNIT")
            .map_err(|source| ImageError::fits(path, source))?
            .map(|unit| unit.trim().to_owned())
            .filter(|unit| !unit.is_empty());
        let channel_count = if plan.dimensions.is_rgb() { 3 } else { 1 };
        let mut planes = ArrayVec::<DecodedPlane, 3>::new();
        for channel in 0..channel_count {
            planes.push(self.read_plane(channel, sections)?);
        }
        let checksum = checksum(sections)?;
        let scan = planes
            .iter()
            .map(|plane| plane.scan)
            .fold(SampleScan::EMPTY, SampleScan::merge);
        if !plan.sample_scale.accepts_maximum(scan.maximum) {
            return Err(ImageError::fits_unsupported(
                path,
                FitsRefusal::UndeclaredFloatScale {
                    maximum: scan.maximum,
                },
            ));
        }
        let header_err = |source| ImageError::fits(path, source);
        let quantization_sigma =
            match domain_keywords::read_quantization_sigma(header).map_err(header_err)? {
                Some(sigma) => Some(sigma),
                None => plan.sample_type.is_integer().then(|| {
                    let step = scan.integer_step(plan.scaling.bscale)
                        / f64::from(plan.sample_scale.divisor);
                    step as f32 * QUANTIZATION_SIGMA_PER_STEP
                }),
            };
        let pedestal = domain_keywords::read_pedestal(header)
            .map_err(header_err)?
            .unwrap_or(context.fits.pedestal);
        let mut metadata = metadata::read(header, plan.sample_type);
        // DATAMAX is a saturation level in the file's sample units, so it only stays comparable to
        // the samples if it is divided by the same span they were.
        metadata.data_max = plan
            .data_max
            .map(|data_max| data_max / f64::from(plan.sample_scale.divisor));
        let saturation = metadata
            .data_max
            .map(|data_max| SATURATION_FRACTION * data_max as f32);
        metadata.saturation_flagged = saturation.is_some()
            || domain_keywords::read_saturation_flagged(header).map_err(header_err)?;
        let flags = self.resolve_flags(&mut planes, saturation)?;
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
                hdu: self.hdu,
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
            // Recorded, not acted on: the rows above were copied in file order whatever this says,
            // and only the Bayer phase was corrected for it. What it buys is that a set mixing the
            // two orders — which loads as mutually mirrored images — can be named as such.
            row_order,
        });

        Ok(DecodedFitsImage {
            metadata,
            cfa_type,
            pixels,
            flags,
        })
    }

    /// Apply the load's null policy to a decoded image's planes — reject the load, or fill the
    /// nulls — and hand back the flags the samples settle: [`QualityFlags::NO_DATA`] where they
    /// were null, and [`QualityFlags::SATURATED`] where a channel reaches `saturation`, a level in
    /// the decoded domain.
    ///
    /// A frame with no nulls and no saturation level — most frames without a `DATAMAX` — returns
    /// before any scan, so the feature costs one sum over at most three integers.
    fn resolve_flags(
        &self,
        planes: &mut [DecodedPlane],
        saturation: Option<f32>,
    ) -> Result<Option<PixelFlags>, ImageError> {
        let path = self.path;
        let dimensions = self.plan.dimensions;
        let policy = self.context.fits.nulls;
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
            // Samples, not pixels: this counts each channel's nulls separately, and the index is in
            // the channel-major sample space the count belongs to. The pixel figure needs the mask,
            // which this branch does not build.
            return Err(ImageError::fits_unsupported(
                path,
                FitsRefusal::NullSamples { count, first_index },
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
        let flags =
            flags.expect("a nonzero count means at least one plane holds a non-finite sample");
        // Only for a frame that has them, and the samples the caller is about to read are partly
        // fill with nothing in the frame itself to say so.
        tracing::info!(
            pixels = flags.count(QualityFlags::NO_DATA),
            of = dimensions.pixel_count(),
            "FITS image declares pixels with no measurement"
        );
        Ok(Some(flags))
    }

    /// Decode plane `channel` through `sections`, a chunk of rows at a time.
    fn read_plane(
        &self,
        channel: usize,
        sections: &mut HduSections<'_>,
    ) -> Result<DecodedPlane, ImageError> {
        let path = self.path;
        let plan = self.plan;
        let context = self.context;
        let width = plan.dimensions.width();
        let height = plan.dimensions.height();
        let expected_pixels = plan.dimensions.pixel_count();
        let mut output = concurrency::zeroed_in_parallel(expected_pixels);
        let mut nulls: Option<NullSummary> = None;
        let mut scan = SampleScan::EMPTY;
        for row_start in (0..height).step_by(plan.rows_per_chunk) {
            context.check_cancelled(path)?;
            let row_end = row_start.saturating_add(plan.rows_per_chunk).min(height);
            let expected_chunk = (row_end - row_start) * width;
            let section = sections
                .read_section(&channel_ranges(plan, channel, row_start..row_end))
                .map_err(|source| ImageError::fits(path, source))?;
            context.check_cancelled(path)?;
            let samples = section.samples();
            if samples.len() != expected_chunk {
                return Err(ImageError::fits_unsupported(
                    path,
                    FitsRefusal::SectionLength {
                        channel,
                        rows: row_start..row_end,
                        samples: samples.len(),
                        expected: expected_chunk,
                    },
                ));
            }
            let start = row_start * width;
            let ChunkScan {
                nulls: chunk_nulls,
                scan: chunk_scan,
            } = convert_and_scan(
                samples,
                section.scaling(),
                plan.sample_scale.divisor,
                &mut output[start..start + expected_chunk],
                &context.cancel,
            )
            .map_err(|Cancelled| ImageError::cancelled(path))?;
            scan = scan.merge(chunk_scan);
            // Each chunk locates its nulls in its own index space; the plane's is what a caller can
            // act on, so the offset is applied here rather than threaded into the pass.
            if let Some(chunk_nulls) = chunk_nulls {
                let chunk_nulls = chunk_nulls.offset_by(start);
                nulls = Some(match nulls {
                    None => chunk_nulls,
                    Some(nulls) => nulls.merge(chunk_nulls),
                });
            }
        }
        Ok(DecodedPlane {
            samples: output,
            nulls,
            scan,
        })
    }
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
/// What sits under a null is not data and [`QualityFlags::NO_DATA`] says so, but the stages that
/// measure a whole plane mostly do not consult the flags, so this value is what they see. The
/// median is the frame's own background level, which leaves a masked region a flat patch instead of
/// the hard-edged hole a zero fill would cut — and a hard edge is what manufactures star detections
/// and drags a background estimate. A deliberate stand-in, not a correction.
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
    // other. The flags say every pixel of it is missing, which is the part that has to survive.
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
    /// The OR of every stored integer but `BLANK`, for an integer `BITPIX`: its trailing zeros are
    /// the ones every sample shares.
    raw_bits: u64,
    /// The largest finite sample after the division.
    maximum: f32,
}

/// A chunk's nulls and its [`SampleScan`].
#[derive(Debug, Clone, Copy, PartialEq)]
struct ChunkScan {
    nulls: Option<NullSummary>,
    scan: SampleScan,
}

impl SampleScan {
    const EMPTY: Self = Self {
        raw_bits: 0,
        maximum: f32::NEG_INFINITY,
    };

    const fn merge(self, other: Self) -> Self {
        Self {
            raw_bits: self.raw_bits | other.raw_bits,
            maximum: self.maximum.max(other.maximum),
        }
    }

    /// One quantization step in physical units: `|BSCALE|·2^z`, where `z` is the trailing zeros
    /// every stored integer shares. 12- or 14-bit data written left-justified into a 16-bit
    /// container steps by 16 or 4 stored units, not by one. The container's own step stands when
    /// every stored integer is zero, and nothing can be read off them.
    fn integer_step(&self, bscale: f64) -> f64 {
        let shared_zeros = if self.raw_bits == 0 {
            0
        } else {
            self.raw_bits.trailing_zeros()
        };
        bscale.abs() * 2.0f64.powi(shared_zeros as i32)
    }
}

/// Convert a section of stored samples into the pipeline's domain in `out`, locate the FITS nulls
/// it carries, and scan it for its [`SampleScan`], in one parallel pass.
///
/// Each sample is `(BZERO + BSCALE·raw) / divisor` in `f64`, narrowed to `f32` once: an
/// intermediate `f32` would round the physical value and then the quotient. The quotient divides
/// rather than multiplying by a reciprocal, which `f64` does not hold exactly for a span like
/// 65535. A NaN or ±inf survives the division, and a finite sample the division pushes past
/// `f32::MAX` narrows to an infinity, so the finite test after it catches nulls and overflows both.
///
/// Each work item runs three plain loops over its samples, which vectorize, rather than one loop
/// whose branches keep it scalar: the stored integers' OR, the conversion, and the finite scan of
/// what it wrote, still in cache.
fn convert_and_scan(
    samples: FitsView<'_>,
    scaling: Scaling,
    divisor: f32,
    out: &mut [f32],
    cancel: &CancelToken,
) -> Result<ChunkScan, Cancelled> {
    let convert = Conversion {
        scaling,
        divisor: f64::from(divisor),
    };
    match samples {
        FitsView::U8(stored) => convert.integers(stored, out, cancel),
        FitsView::I16(stored) => convert.integers(stored, out, cancel),
        FitsView::I32(stored) => convert.integers(stored, out, cancel),
        FitsView::I64(stored) => convert.integers(stored, out, cancel),
        FitsView::F32(stored) => convert.floats(stored, out, cancel),
        FitsView::F64(stored) => convert.floats(stored, out, cancel),
    }
}

/// The scaling and the divisor one section converts by.
#[derive(Debug, Clone, Copy)]
struct Conversion {
    scaling: Scaling,
    divisor: f64,
}

impl Conversion {
    /// Integer samples, the `BLANK` sentinel a NaN and no part of the stored integers' OR.
    fn integers<T: Copy + Sync + Into<i64>>(
        self,
        stored: &[T],
        out: &mut [f32],
        cancel: &CancelToken,
    ) -> Result<ChunkScan, Cancelled> {
        let Scaling {
            bscale,
            bzero,
            blank,
        } = self.scaling;
        // A sentinel no stored value matches when the HDU declares none.
        let blank = blank.unwrap_or(i64::MIN);
        let divisor = self.divisor;
        self.each_chunk(stored, out, cancel, |stored, out| {
            let mut raw_bits = 0u64;
            for &raw in stored {
                let raw: i64 = raw.into();
                // Two's complement keeps a negative value's trailing zeros.
                raw_bits |= if raw == blank { 0 } else { raw.cast_unsigned() };
            }
            for (pixel, &raw) in out.iter_mut().zip(stored) {
                let raw: i64 = raw.into();
                let physical = bzero + bscale * raw as f64;
                let value = (physical / divisor) as f32;
                *pixel = if raw == blank { f32::NAN } else { value };
            }
            raw_bits
        })
    }

    /// Floating-point samples, which carry their own nulls.
    fn floats<T: Copy + Sync + Into<f64>>(
        self,
        stored: &[T],
        out: &mut [f32],
        cancel: &CancelToken,
    ) -> Result<ChunkScan, Cancelled> {
        let Scaling { bscale, bzero, .. } = self.scaling;
        let divisor = self.divisor;
        self.each_chunk(stored, out, cancel, |stored, out| {
            for (pixel, &raw) in out.iter_mut().zip(stored) {
                *pixel = ((bzero + bscale * raw.into()) / divisor) as f32;
            }
            0
        })
    }

    /// `convert` over each work item, returning its stored integers' OR, then the finite scan of
    /// what it wrote, reduced over the section.
    fn each_chunk<T: Copy + Sync>(
        self,
        stored: &[T],
        out: &mut [f32],
        cancel: &CancelToken,
        convert: impl Fn(&[T], &mut [f32]) -> u64 + Sync,
    ) -> Result<ChunkScan, Cancelled> {
        Cancelled::check(cancel)?;
        debug_assert_eq!(stored.len(), out.len());
        out.par_chunks_mut(CHUNK_SAMPLES)
            .zip(stored.par_chunks(CHUNK_SAMPLES))
            .enumerate()
            .map(|(chunk_index, (out, stored))| {
                Cancelled::check(cancel)?;
                let raw_bits = convert(stored, out);
                let mut nonfinite = false;
                let mut maximum = f32::NEG_INFINITY;
                for &pixel in out.iter() {
                    let finite = pixel.is_finite();
                    nonfinite |= !finite;
                    maximum = maximum.max(if finite { pixel } else { f32::NEG_INFINITY });
                }
                Ok(ChunkScan {
                    nulls: nonfinite.then(|| summarize_nulls(out, chunk_index * CHUNK_SAMPLES)),
                    scan: SampleScan { raw_bits, maximum },
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
}

/// Count a chunk's nulls and locate the first, in the whole-span index space `chunk_start` anchors
/// it to.
///
/// Off the hot path by construction: [`Conversion::each_chunk`] only reaches this once a chunk is
/// known to hold at least one null.
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
mod tests;
