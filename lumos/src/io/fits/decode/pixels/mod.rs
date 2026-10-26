use std::io::{Read, Seek};
use std::mem;

use arrayvec::ArrayVec;
use common::{CancelToken, par};
use fits_well::ImageReader;
use fits_well::header::{Header, keywords};
use fits_well::image::{Scaling, View as FitsView};
use fits_well::io::StreamSource;
use imaginarium::PlanarImage;
use rayon::prelude::*;

use crate::error::Cancelled;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::image::flags::quality_flags::{QualityFlags, SATURATION_FRACTION};
use crate::image::metadata::fits_checksum_provenance::FitsChecksumProvenance;
use crate::image::metadata::fits_transfer_provenance::{FitsHduProvenance, FitsTransferProvenance};
use crate::image::metadata::image_provenance::{
    ColorProvenance, DemosaicProvenance, ImageProvenance, SourceProvenance,
};
use crate::image::metadata::sample_domain::SampleDomain;
use crate::io::fits::decode::DecodedFitsImage;
use crate::io::fits::decode::plan::{self, FitsDecodePlan};
use crate::io::fits::error::{FitsLoadError, FitsRefusal};
use crate::io::fits::fits_load_config::FitsChecksumPolicy;
use crate::io::fits::metadata;
use crate::io::fits::metadata::domain_keywords;
use crate::io::fits::tile_parallel;
use crate::io::load_ctx::LoadCtx;
use crate::io::null_policy::{CountedPlane, PlaneNulls};
use crate::noise::QUANTIZATION_SIGMA_PER_STEP;

/// The decode of one image HDU: the HDU, its header and the plan its preflight made, and the file
/// it is read from under the load's context.
#[derive(Debug)]
pub(super) struct HduDecode<'a> {
    pub(super) hdu: FitsHduProvenance,
    pub(super) header: &'a Header,
    pub(super) plan: &'a FitsDecodePlan,
    /// The pattern the header declares, which the selection resolved.
    pub(super) cfa_type: Option<CfaType>,
    pub(super) ctx: &'a LoadCtx,
}

impl HduDecode<'_> {
    /// Decode the HDU from `reader`, its checksum checked under `policy` from the bytes the decode
    /// reads, and judged before anything is returned: a refused checksum fails the load as before
    /// the decode it would, at the cost of the decode.
    pub(super) fn read_stream<R: Read + Seek>(
        self,
        image: ImageReader<'_, StreamSource<R>>,
        policy: FitsChecksumPolicy,
    ) -> Result<DecodedFitsImage, FitsLoadError> {
        let idx = self.hdu.idx;
        let ctx = self.ctx;
        if policy == FitsChecksumPolicy::Ignore {
            return self.read(image, |_| Ok(FitsChecksumProvenance::NOT_CHECKED));
        }
        self.read(image.with_checksums(), |image| {
            Cancelled::check(&ctx.cancel)?;
            let report = image.finish()?;
            policy.judge(report, idx)
        })
    }

    /// Decode the HDU through `image`, and take its checksum provenance from `checksum` once the
    /// planes are read.
    pub(super) fn read<'r, R: Read + Seek>(
        self,
        mut image: ImageReader<'r, StreamSource<R>>,
        checksum: impl FnOnce(
            ImageReader<'r, StreamSource<R>>,
        ) -> Result<FitsChecksumProvenance, FitsLoadError>,
    ) -> Result<DecodedFitsImage, FitsLoadError> {
        let Self {
            header,
            plan,
            cfa_type,
            ctx,
            ..
        } = self;
        tracing::debug!(
            source_bytes = plan.source_bytes,
            decoded_bytes = plan.decoded_bytes,
            peak_bytes = plan.peak_bytes,
            "FITS image passed header-first memory preflight"
        );
        // The keywords that can fail a load are read before a plane is: a frame they refuse costs
        // no decode.
        let row_order = metadata::read_row_order(header)?;
        // The unit is half the sample domain, which decides whether frames combine. An all-blank
        // BUNIT parses to the single significant space §4.2.1.1 requires, which states no unit
        // rather than an empty one — left alone it would disagree with every real unit. Surrounding
        // blanks are a writer artifact rather than part of a unit name, so both ends go, and the
        // domain comparison downstream is then plain equality — see `SampleDomain::conversion_to`
        // for why it stops there and does not fold case.
        let unit = metadata::read_text(header, keywords::BUNIT)?
            .map(|unit| unit.trim().to_owned())
            .filter(|unit| !unit.is_empty());
        let mut planes = self.read_planes(&mut image)?;
        let checksum = checksum(image)?;
        let scan = planes
            .iter()
            .map(|plane| plane.scan)
            .fold(SampleScan::EMPTY, SampleScan::merge);
        if !plan.sample_scale.accepts_maximum(scan.maximum) {
            return Err(FitsLoadError::from(FitsRefusal::UndeclaredFloatScale {
                maximum: scan.maximum,
            }));
        }
        let quantization_sigma = match domain_keywords::read_quantization_sigma(header)? {
            Some(sigma) => Some(sigma),
            None => plan.sample_type.is_integer().then(|| {
                let step = scan.integer_step(plan.scaling.bscale) / plan.sample_scale.divisor;
                step as f32 * QUANTIZATION_SIGMA_PER_STEP
            }),
        };
        let pedestal = domain_keywords::read_pedestal(header)?.unwrap_or(ctx.fits.pedestal);
        let mut metadata = metadata::read(header, plan.sample_type);
        // DATAMAX is a saturation level in the file's sample units, so it only stays comparable to
        // the samples if it is divided by the same span they were.
        metadata.header.data_max = plan
            .data_max
            .map(|data_max| data_max / plan.sample_scale.divisor);
        let saturation = metadata
            .header
            .data_max
            .map(|data_max| SATURATION_FRACTION * data_max as f32);
        let saturation_flagged =
            saturation.is_some() || domain_keywords::read_saturation_flagged(header)?;
        let flags = self.resolve_flags(&mut planes, saturation)?;
        let pixels = PlanarImage::from_planar_channels(
            plan.desc,
            planes.into_iter().map(|plane| plane.samples),
        );
        let provenance = ImageProvenance {
            source: SourceProvenance::Fits(FitsTransferProvenance {
                bscale: plan.scaling.bscale,
                bzero: plan.scaling.bzero,
                hdu: self.hdu,
                checksum,
            }),
            color: if cfa_type.and_then(CfaType::mosaic).is_some() {
                ColorProvenance::SensorCfa
            } else if plan.desc.is_mono() {
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
        };
        metadata.derived = mem::take(&mut metadata.derived)
            .rebuild()
            .saturation_flagged(saturation_flagged)
            .domain(SampleDomain {
                scale: plan.sample_scale.divisor,
                origin: plan.sample_scale.origin,
                pedestal,
                unit,
            })
            .quantization_sigma(quantization_sigma)
            .provenance(provenance)
            .build();

        Ok(DecodedFitsImage {
            metadata,
            pixels,
            flags,
            flat_gain: None,
        })
    }

    /// The load's null policy applied to a decoded image's planes, with the nulls the decode
    /// counted and the saturation level — see
    /// [`NullPolicy::resolve`](crate::NullPolicy::resolve).
    fn resolve_flags(
        &self,
        planes: &mut [DecodedPlane],
        saturation: Option<f32>,
    ) -> Result<Option<PixelFlags>, FitsLoadError> {
        let desc = self.plan.desc;
        let has_nulls = planes.iter().any(|plane| plane.nulls.is_some());
        let mut counted: ArrayVec<CountedPlane<'_>, 3> = planes
            .iter_mut()
            .map(|plane| CountedPlane {
                samples: &mut plane.samples,
                nulls: plane.nulls,
            })
            .collect();
        let flags = self
            .ctx
            .nulls
            .resolve(&mut counted, desc.size(), saturation)?;
        if has_nulls {
            // Only for a frame that has them, and the samples the caller is about to read are
            // partly fill with nothing in the frame itself to say so.
            tracing::info!(
                pixels = flags
                    .as_ref()
                    .map_or(0, |flags| flags.count(QualityFlags::NO_DATA)),
                of = desc.pixel_count(),
                "FITS image declares pixels with no measurement"
            );
        }
        Ok(flags)
    }

    /// Decode every plane from `image`, a chunk of rows of all of them at a time, each swapped
    /// into one buffer of words. A section is plane-major, so each plane's samples are one run of
    /// the chunk, and a tile that spans planes decodes once.
    fn read_planes<R: Read + Seek>(
        &self,
        image: &mut ImageReader<'_, StreamSource<R>>,
    ) -> Result<ArrayVec<DecodedPlane, 3>, FitsLoadError> {
        let plan = self.plan;
        let ctx = self.ctx;
        let width = plan.desc.width();
        let channels = plan.desc.channel_count();
        let mut planes: ArrayVec<DecodedPlane, 3> = (0..channels)
            .map(|_| DecodedPlane {
                samples: zeroed_plane(plan.desc.pixel_count()),
                nulls: None,
                scan: SampleScan::EMPTY,
            })
            .collect();
        let mut words = Vec::new();
        for ranges in plan::chunk_regions(&plan.shape, plan.rows_per_chunk) {
            Cancelled::check(&ctx.cancel)?;
            let rows = ranges[1].clone();
            let plane_chunk = rows.len() * width;
            let section = tile_parallel::view(image, ranges.as_slice(), &mut words)?;
            Cancelled::check(&ctx.cancel)?;
            let samples = section.samples();
            if samples.len() != plane_chunk * channels {
                return Err(FitsLoadError::from(FitsRefusal::SectionLen {
                    rows,
                    samples: samples.len(),
                    expected: plane_chunk * channels,
                }));
            }
            let start = rows.start * width;
            let conversion = Conversion {
                scaling: section.scaling(),
                divisor: plan.sample_scale.divisor,
                cancel: &ctx.cancel,
            };
            for (channel, plane) in planes.iter_mut().enumerate() {
                let run = channel * plane_chunk..(channel + 1) * plane_chunk;
                let ChunkScan {
                    nulls: chunk_nulls,
                    scan: chunk_scan,
                } = conversion.scan(
                    samples.slice(run),
                    &mut plane.samples[start..start + plane_chunk],
                )?;
                plane.scan = plane.scan.merge(chunk_scan);
                // Each chunk locates its nulls in its own index space; the plane's is what a
                // caller can act on, so the offset is applied here rather than threaded into the
                // pass.
                if let Some(chunk_nulls) = chunk_nulls {
                    let chunk_nulls = chunk_nulls.offset_by(start);
                    plane.nulls = Some(match plane.nulls {
                        None => chunk_nulls,
                        Some(nulls) => nulls.merge(chunk_nulls),
                    });
                }
            }
        }
        Ok(planes)
    }
}

/// One decoded channel plane, and where its nulls are.
///
/// The count and first index come out of the pass that scaled the samples, which already walked
/// every one of them; locating them a second time to decide policy would be a second full scan.
#[derive(Debug)]
struct DecodedPlane {
    samples: Vec<f32>,
    nulls: Option<PlaneNulls>,
    scan: SampleScan,
}

/// Samples per parallel work item in the per-chunk passes below.
const CHUNK_SAMPLES: usize = 64 * 1024;

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
    nulls: Option<PlaneNulls>,
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

/// The scaling and the divisor one section converts by, and the token that stops it between work
/// items.
#[derive(Debug, Clone, Copy)]
struct Conversion<'a> {
    scaling: Scaling,
    divisor: f64,
    cancel: &'a CancelToken,
}

impl Conversion<'_> {
    /// Convert a section of stored samples into the pipeline's domain in `out`, locate the FITS
    /// nulls it carries, and scan it for its [`SampleScan`], in one parallel pass.
    ///
    /// Each sample is `(BZERO + BSCALE·raw) / divisor` in `f64`, narrowed to `f32` once: an
    /// intermediate `f32` would round the physical value and then the quotient. The quotient
    /// divides rather than multiplying by a reciprocal, which `f64` does not hold exactly for a
    /// span like
    /// 65535. A NaN or ±inf survives the division, and a finite sample the division pushes past
    ///        `f32::MAX` narrows to an infinity, so the finite test after it catches nulls and
    ///        overflows both.
    ///
    /// Each work item runs three plain loops over its samples, which vectorize, rather than one
    /// loop whose branches keep it scalar: the stored integers' OR, the conversion, and the finite
    /// scan of what it wrote, still in cache.
    fn scan(self, samples: FitsView<'_>, out: &mut [f32]) -> Result<ChunkScan, Cancelled> {
        match samples {
            FitsView::U8(stored) => self.integers(stored, out),
            FitsView::I16(stored) => self.integers(stored, out),
            FitsView::I32(stored) => self.integers(stored, out),
            FitsView::I64(stored) => self.integers(stored, out),
            FitsView::F32(stored) => self.floats(stored, out),
            FitsView::F64(stored) => self.floats(stored, out),
        }
    }

    /// Integer samples, the `BLANK` sentinel a NaN and no part of the stored integers' OR. Every
    /// stored value is a sample when the HDU declares no `BLANK`, `i64::MIN` included.
    fn integers<T: Copy + Sync + Into<i64>>(
        self,
        stored: &[T],
        out: &mut [f32],
    ) -> Result<ChunkScan, Cancelled> {
        let Scaling {
            bscale,
            bzero,
            blank,
        } = self.scaling;
        let divisor = self.divisor;
        let physical = move |raw: i64| ((bzero + bscale * raw as f64) / divisor) as f32;
        match blank {
            Some(blank) => self.each_chunk(stored, out, |stored, out| {
                let mut raw_bits = 0u64;
                for &raw in stored {
                    let raw: i64 = raw.into();
                    // Two's complement keeps a negative value's trailing zeros.
                    raw_bits |= if raw == blank { 0 } else { raw.cast_unsigned() };
                }
                for (pixel, &raw) in out.iter_mut().zip(stored) {
                    let raw: i64 = raw.into();
                    *pixel = if raw == blank {
                        f32::NAN
                    } else {
                        physical(raw)
                    };
                }
                raw_bits
            }),
            None => self.each_chunk(stored, out, |stored, out| {
                let mut raw_bits = 0u64;
                for &raw in stored {
                    raw_bits |= raw.into().cast_unsigned();
                }
                for (pixel, &raw) in out.iter_mut().zip(stored) {
                    *pixel = physical(raw.into());
                }
                raw_bits
            }),
        }
    }

    /// Floating-point samples, which carry their own nulls.
    fn floats<T: Copy + Sync + Into<f64>>(
        self,
        stored: &[T],
        out: &mut [f32],
    ) -> Result<ChunkScan, Cancelled> {
        let Scaling { bscale, bzero, .. } = self.scaling;
        let divisor = self.divisor;
        self.each_chunk(stored, out, |stored, out| {
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
        convert: impl Fn(&[T], &mut [f32]) -> u64 + Sync,
    ) -> Result<ChunkScan, Cancelled> {
        let cancel = self.cancel;
        Cancelled::check(cancel)?;
        debug_assert_eq!(stored.len(), out.len());
        par::chunks_mut(out, CHUNK_SAMPLES)
            .zip(par::chunks(stored, CHUNK_SAMPLES))
            .enumerate()
            .map(|(chunk_idx, (out, stored))| {
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
                    nulls: nonfinite
                        .then(|| PlaneNulls::of(out, chunk_idx * CHUNK_SAMPLES))
                        .flatten(),
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

/// The `f32` samples of a 2 MiB transparent huge page.
const HUGE_PAGE_SAMPLES: usize = 2 * 1024 * 1024 / size_of::<f32>();

/// `len` zeros, each 2 MiB page first written by one worker, for a plane the conversion then fills
/// in chunks smaller than a page. A fresh plane's pages are zeroed by the kernel at the first write
/// to each; when the chunks make those first writes, several workers fault on one huge page and
/// wait on each other, which made a 24 MP decode 50 % slower.
fn zeroed_plane(len: usize) -> Vec<f32> {
    let mut plane = vec![0.0; len];
    par::chunks_mut(&mut plane, HUGE_PAGE_SAMPLES).for_each(|block| block.fill(0.0));
    plane
}

#[cfg(test)]
mod tests;
