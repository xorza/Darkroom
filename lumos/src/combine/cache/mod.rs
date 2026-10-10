//! Chunked combine engine for resident and memory-mapped stacking frames.

pub(crate) mod core;
pub(crate) mod frame_check;
pub(crate) mod frame_weights;
pub(crate) mod loader;
pub(crate) mod sample;
pub(crate) mod sample_noise;
pub(crate) mod set_facts;
pub(crate) mod slots;

use common::CancelToken;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::combine::cache::core::{CacheCore, CacheTier, ChunkContext};
use crate::combine::cache::frame_check::FrameCheck;
use crate::combine::cache::frame_weights::FrameWeights;
use crate::combine::cache::loader::LoadedCache;
use crate::combine::cache::sample::{
    CombineScratch, CombinedSample, GatheredSamples, PixelSamples,
};
use crate::combine::cache::sample_noise::{NoiseColumns, SampleNoise};
use crate::combine::cache::set_facts::SetFacts;
use crate::combine::cache::slots::Slots;
use crate::combine::config::{Normalization, StackConfig};
use crate::combine::error::StackError;
use crate::combine::normalization::FrameNorm;
use crate::combine::pixel_coverage::PixelCoverage;
use crate::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::combine::stack::StackFrame;
use crate::concurrency::job_scratch_pool::JobScratchPool;
use crate::error::FrameDimensionMismatch;
use crate::frame_store::capture_conditions::CaptureConditions;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_plane::StoredPlane;
use crate::ingest::frame_step::FrameStep;
use crate::ingest::ingest_run::IngestRun;
use crate::io::cancelled::Cancelled;
use crate::io::image::cfa::CfaImage;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};
use crate::io::image::unverified_conditions::UnverifiedConditions;
use crate::math::vec2us::Vec2us;
use crate::memory::chunk_memory_layout::ChunkMemoryLayout;
use crate::progress::progress_callback::ProgressCallback;
use crate::run_report::{AtomicFlagCounts, LocalFlagCounts, RunReport};
use crate::stack_product::StackProduct;
use crate::stack_product::coverage::Coverage;
use crate::stack_product::quality_map::QualityMap;
use crate::stack_product::quality_planes::QualityPlanes;
use std::path::Path;

/// Channel-shaped result of one combine pass. A plane is `None` when [`QualityPlanes`] did not
/// ask for it.
#[derive(Debug)]
pub(crate) struct CombineOutput {
    pub(super) pixels: LinearPixels,
    weight: Option<LinearPixels>,
    variance: Option<LinearPixels>,
    dispersion: Option<LinearPixels>,
    /// The stack's flags, for a frame set where any frame carries flags: [`QualityFlags::NO_DATA`] where
    /// no frame reached a pixel, [`QualityFlags::SATURATED`] where a kept sample was.
    flags: Option<Buffer2<u8>>,
    report: RunReport,
}

/// The output rows one combine row-task writes: the combined value, plus whichever ancillary
/// planes were requested. Bundling them keeps one gather loop instead of one per plane subset.
#[derive(Debug)]
struct QualityRows<'a> {
    value: &'a mut [f32],
    weight: Option<&'a mut [f32]>,
    variance: Option<&'a mut [f32]>,
    dispersion: Option<&'a mut [f32]>,
    flags: Option<&'a mut [u8]>,
}

/// What a combine pass gathers beside the samples, and how many samples it keeps at the least.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CombineRequest<'a> {
    /// Per-frame weights; `None` weighs every frame equally.
    pub(crate) weights: Option<&'a FrameWeights>,
    pub(crate) planes: QualityPlanes,
    pub(crate) min_survivors: usize,
    /// The frames' noise, for a reducer that measures a spread or a variance; `None` gathers no
    /// noise.
    pub(crate) noise: Option<&'a SampleNoise>,
    /// How `weights` and `noise` index a pixel.
    pub(crate) slots: Slots,
}

/// The frames feeding one combine, with their normalization parameters. Calibration masters and
/// registered light stacks share it; what separates them is only whether their frames carry quality
/// planes, and even an unwarped one does when its source declared pixels with no measurement.
#[derive(Debug)]
pub(crate) struct FrameCache {
    pub(crate) frames: Vec<StoredFrame>,
    /// Each frame's affine onto the reference, measured once at construction for the
    /// normalization the cache was built with; `None` when every frame is combined as it stands.
    pub(crate) frame_norms: Option<Vec<FrameNorm>>,
    pub(crate) core: CacheCore,
}

impl FrameCache {
    /// Build a cache from frames already placed in the shared frame store.
    pub(crate) fn from_stored_frames(
        frames: Vec<StoredFrame>,
        core: CacheCore,
        normalization: Normalization,
    ) -> Result<Self, StackError> {
        // The pipeline produced these frames: their geometry, samples and quality pair are its own
        // contracts, checked in debug builds only — on the spill tier a release check would fault
        // every plane in from disk once before the combine reads it again. What the frames' sources
        // stated (domain, row order, pattern) is the input's, and is checked here always.
        let mut facts = SetFacts::default();
        for (index, frame) in frames.iter().enumerate() {
            Cancelled::check(&core.cancel)?;
            facts.admit(index, &frame.source_stats.facts)?;
            debug_assert!(
                FrameCheck {
                    index,
                    cancel: &CancelToken::never(),
                }
                .stored(frame, core.dimensions, &mut SetFacts::default())
                .is_ok(),
                "stored frame {index} breaks the pipeline's own frame contract"
            );
        }
        let frame_norms =
            FrameNorm::measure(&frames, core.dimensions, normalization, &core.cancel)?;
        Ok(Self {
            frames,
            frame_norms,
            core,
        })
    }

    /// Build an in-memory frame-quality-aware cache from [`StackFrame`]s.
    pub(crate) fn from_stack_frames(
        frames: Vec<StackFrame>,
        normalization: Normalization,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<Self, StackError> {
        debug_assert!(!frames.is_empty(), "`combine_cached` refuses an empty set");
        Cancelled::check(&cancel)?;
        let dimensions = frames[0].image.dimensions();
        let metadata = frames[0].image.metadata.clone();

        // Width and height before the shared check, which a stored plane can only compare by
        // sample count: a 4×2 frame in a 2×4 set has the right count and the wrong shape.
        for (index, frame) in frames.iter().enumerate() {
            FrameDimensionMismatch::check(index, dimensions, frame.image.dimensions())?;
            for (kind, plane) in frame.quality.present() {
                if (plane.width(), plane.height()) != (dimensions.width(), dimensions.height()) {
                    return Err(StackError::WarpPlaneDimensionMismatch {
                        index,
                        plane: kind,
                        expected_width: dimensions.width(),
                        expected_height: dimensions.height(),
                        actual_width: plane.width(),
                        actual_height: plane.height(),
                    });
                }
            }
        }
        let stored = frames
            .into_iter()
            .map(|frame| StoredFrame::from_memory(frame.image, frame.quality, frame.source_stats))
            .collect::<Vec<_>>();
        let mut facts = SetFacts::default();
        for (index, frame) in stored.iter().enumerate() {
            FrameCheck {
                index,
                cancel: &cancel,
            }
            .stored(frame, dimensions, &mut facts)?;
        }
        let frame_norms = FrameNorm::measure(&stored, dimensions, normalization, &cancel)?;

        Ok(Self {
            frames: stored,
            frame_norms,
            core: CacheCore {
                tier: CacheTier::Resident,
                dimensions,
                metadata,
                progress,
                cancel,
            },
        })
    }

    /// Assemble the combined image, geometric coverage, and per-channel survivor quality.
    pub(crate) fn finish_product(
        &self,
        combined: CombineOutput,
        planes: QualityPlanes,
        quantization_sigma: Option<f32>,
    ) -> StackProduct {
        let CombineOutput {
            pixels,
            weight: weight_pixels,
            variance: variance_pixels,
            dispersion: dispersion_pixels,
            flags,
            report,
        } = combined;
        let dimensions = self.core.dimensions;
        // Every frame carries the first one's pattern: `SetFacts` held them to it.
        let cfa_type = self.frames[0].source_stats.facts.cfa_type;
        // Every saturated sample of the stack is flagged only when every frame's was.
        let saturation_flagged = self
            .frames
            .iter()
            .all(|frame| frame.source_stats.facts.saturation_flagged);
        let conditions = CaptureConditions::shared(
            self.frames
                .iter()
                .map(|frame| frame.source_stats.facts.conditions),
        );
        let unverified_dark = self
            .frames
            .iter()
            .map(|frame| frame.source_stats.facts.unverified_dark)
            .fold(UnverifiedConditions::NONE, UnverifiedConditions::union);
        let image = LinearImage {
            // The reference frame's metadata, with the combine's own quantization σ, saturation
            // record, capture conditions and unverified dark match, and no mosaic noise: what the
            // reference's decoder and demosaic recorded describes one frame, not the stack.
            metadata: ImageMetadata {
                quantization_sigma,
                mosaic_noise: None,
                saturation_flagged,
                exposure_time: conditions.exposure_time,
                ccd_temp: conditions.ccd_temp,
                unverified_dark,
                ..self.core.metadata.clone()
            },
            pixels,
            flags: flags.and_then(PixelFlags::from_buffer),
        };
        let weight = weight_pixels.map(QualityMap::from_pixels);
        let variance = variance_pixels.map(QualityMap::from_pixels);
        let dispersion = dispersion_pixels.map(QualityMap::from_pixels);
        let frame_count = self.frames.len();
        let width = dimensions.width();
        let height = dimensions.height();

        // No frame carries support, so every pixel is fully covered — and saying so costs one
        // number rather than an image-sized plane of `1.0`.
        if !planes.coverage || self.frames.iter().all(|frame| frame.quality.is_none()) {
            return StackProduct {
                image,
                coverage: planes.coverage.then(|| Coverage::Uniform {
                    value: 1.0,
                    size: dimensions.size(),
                }),
                weight,
                variance,
                dispersion,
                cfa_type,
                report,
            };
        }

        let mut coverage = Buffer2::new_default(width, height);
        let inv_frames = 1.0 / frame_count as f32;

        // Coverage planes share their frame's tier, so they may be mmap-backed: read them in the
        // same row-aligned chunks the combine uses, against the figure the combine sized against.
        let chunk_rows = self
            .core
            .tier
            .chunk_memory()
            .map_or(height, |chunk_memory| {
                self.coverage_layout(planes)
                    .optimal_chunk_rows(dimensions.size(), chunk_memory)
            });

        let mut start_row = 0;
        while start_row < height {
            let end_row = (start_row + chunk_rows).min(height);
            let base = start_row * width;
            let span = (end_row - start_row) * width;

            let cov_chunks =
                self.quality_chunks(|frame| frame.quality.coverage(), base, base + span);

            let cov_out = &mut coverage.pixels_mut()[base..base + span];
            cov_out
                .par_chunks_mut(width)
                .enumerate()
                .for_each(|(row_in_chunk, cov_row)| {
                    let row_base = row_in_chunk * width;
                    for (px, output) in cov_row.iter_mut().enumerate() {
                        let local = row_base + px;
                        // The combine's own gate, so the fraction reported here is exactly the
                        // fraction of frames whose sample entered the statistics.
                        let count = cov_chunks
                            .iter()
                            .filter(|cov| {
                                cov.map_or(PixelCoverage::FULL, |map| {
                                    PixelCoverage::new(map[local])
                                })
                                .contributes()
                            })
                            .count();
                        *output = count as f32 * inv_frames;
                    }
                });

            start_row = end_row;
        }

        StackProduct {
            image,
            coverage: Some(Coverage::PerPixel(coverage)),
            weight,
            variance,
            dispersion,
            cfa_type,
            report,
        }
    }

    /// The combine: for each output pixel, gather the frames that cover it, hand them to
    /// `combine`, and write the reduced value plus whichever [`QualityPlanes`] were requested.
    ///
    /// [`PixelCoverage`] alone decides whether a frame is gathered at a pixel; its confidence then
    /// scales the weight the sample carries into the reduction, and is guaranteed positive wherever
    /// the frame was gathered. A frame carrying neither plane contributes everywhere at unit
    /// confidence, which is what lets calibration masters and registered light stacks share this
    /// loop. A pixel no frame supports gets `0`.
    pub(crate) fn process_chunked<Combine>(
        &self,
        request: CombineRequest<'_>,
        combine: Combine,
    ) -> CombineOutput
    where
        Combine: Fn(PixelSamples<'_>, &mut ScratchBuffers) -> CombinedSample + Sync,
    {
        let CombineRequest {
            weights,
            planes,
            min_survivors,
            noise,
            slots,
        } = request;
        debug_assert!(noise.is_none_or(|noise| noise.slots() == slots));
        // An in-memory stack is one chunk, so the per-chunk cancel check in
        // `process_chunks` can't interrupt the combine — poll per row here too.
        let cancel = self.core.cancel.clone();
        let frame_norms = self.frame_norms.as_deref();
        let dimensions = self.core.dimensions;
        let memory = self.weighted_layout(planes);
        // Coverage sizing must reuse this pre-output snapshot or resident planes are charged twice.
        let mut output_weight = planes.weight.then(|| LinearPixels::new_zeroed(dimensions));
        let mut output_variance = planes
            .variance
            .then(|| LinearPixels::new_zeroed(dimensions));
        let mut output_dispersion = planes
            .dispersion
            .then(|| LinearPixels::new_zeroed(dimensions));
        let any_flags = self.frames.iter().any(|frame| frame.flags.is_some());
        let mut output_flags =
            any_flags.then(|| Buffer2::<u8>::new_default(dimensions.width(), dimensions.height()));
        let excluded = AtomicFlagCounts::default();
        let kept_flagged = AtomicFlagCounts::default();
        // One pool for the whole combine. `process_chunks` invokes the row loop below once per
        // chunk per channel, so the leases have to outlive any single `for_each_init`.
        let scratch_pool = JobScratchPool::<CombineScratch>::default();
        let pixels = self.core.process_chunks(
            &self.frames,
            memory,
            self.core.tier.chunk_memory(),
            |output_slice, ctx| {
                let ChunkContext {
                    frames,
                    width,
                    channel,
                    pixel_offset,
                } = ctx;
                let frame_count = frames.len();
                let chunk_pixels = output_slice.len();
                debug_assert_eq!(pixel_offset % width, 0, "a chunk starts at a row");
                // Per-frame support and confidence slices; `None` means full support/unit
                // confidence.
                let chunk_end = pixel_offset + chunk_pixels;
                let coverage =
                    self.quality_chunks(|frame| frame.quality.coverage(), pixel_offset, chunk_end);
                let confidence = self.quality_chunks(
                    |frame| frame.quality.confidence(),
                    pixel_offset,
                    chunk_end,
                );
                let flags: Vec<Option<&[u8]>> = self
                    .frames
                    .iter()
                    .map(|frame| {
                        frame
                            .flags
                            .as_ref()
                            .map(|plane| plane.chunk(pixel_offset, chunk_end))
                    })
                    .collect();
                // One row bundle per output row, so an unrequested plane simply has no slice
                // to write instead of needing its own copy of the gather loop below.
                let mut rows: Vec<QualityRows<'_>> = output_slice
                    .chunks_mut(width)
                    .map(|value| QualityRows {
                        value,
                        weight: None,
                        variance: None,
                        dispersion: None,
                        flags: None,
                    })
                    .collect();
                if let Some(plane) = output_weight.as_mut() {
                    let slice = &mut plane.channel_mut(channel).pixels_mut()
                        [pixel_offset..pixel_offset + chunk_pixels];
                    for (row, chunk) in rows.iter_mut().zip(slice.chunks_mut(width)) {
                        row.weight = Some(chunk);
                    }
                }
                if let Some(plane) = output_variance.as_mut() {
                    let slice = &mut plane.channel_mut(channel).pixels_mut()
                        [pixel_offset..pixel_offset + chunk_pixels];
                    for (row, chunk) in rows.iter_mut().zip(slice.chunks_mut(width)) {
                        row.variance = Some(chunk);
                    }
                }
                if let Some(plane) = output_dispersion.as_mut() {
                    let slice = &mut plane.channel_mut(channel).pixels_mut()
                        [pixel_offset..pixel_offset + chunk_pixels];
                    for (row, chunk) in rows.iter_mut().zip(slice.chunks_mut(width)) {
                        row.dispersion = Some(chunk);
                    }
                }
                // One plane for every channel, each pass ORing into it: a pixel is flagged when any
                // of its channels is.
                if let Some(plane) = output_flags.as_mut() {
                    let slice = &mut plane.pixels_mut()[pixel_offset..pixel_offset + chunk_pixels];
                    for (row, chunk) in rows.iter_mut().zip(slice.chunks_mut(width)) {
                        row.flags = Some(chunk);
                    }
                }
                rows.into_par_iter().enumerate().for_each_init(
                    || {
                        let mut scratch = scratch_pool.acquire();
                        scratch.resize(frame_count);
                        scratch
                    },
                    |scratch, (row_in_chunk, mut row)| {
                        // Cancelled: skip the row's work (output stays zero; the
                        // caller discards the partial result and reports Cancelled).
                        if cancel.is_cancelled() {
                            return;
                        }
                        // One deref, then disjoint field borrows — `combine` takes two of them
                        // at once, which `scratch.values` / `scratch.buffers` through the lease's
                        // `DerefMut` could not provide.
                        let CombineScratch {
                            values,
                            eff_weights,
                            sample_flags,
                            frame_ids,
                            noise_background,
                            noise_sky,
                            noise_inverse_electrons,
                            buffers,
                        } = &mut **scratch;
                        let values = values.as_mut_slice();
                        let eff_weights = eff_weights.as_mut_slice();
                        let sample_flags = sample_flags.as_mut_slice();
                        let frame_ids = frame_ids.as_mut_slice();
                        let noise_background = noise_background.as_mut_slice();
                        let noise_sky = noise_sky.as_mut_slice();
                        let noise_inverse_electrons = noise_inverse_electrons.as_mut_slice();
                        let mut row_excluded = LocalFlagCounts::default();
                        let mut row_kept = LocalFlagCounts::default();
                        let row_offset = row_in_chunk * width;
                        let y = pixel_offset / width + row_in_chunk;
                        for pixel_in_row in 0..width {
                            let pixel_idx = row_offset + pixel_in_row;
                            let slot = slots.slot(channel, Vec2us::new(pixel_in_row, y));
                            let mut covered = 0usize;
                            for (frame_idx, chunk) in frames.iter().enumerate() {
                                let support = match coverage[frame_idx] {
                                    Some(map) => PixelCoverage::new(map[pixel_idx]),
                                    None => PixelCoverage::FULL,
                                };
                                let q = match confidence[frame_idx] {
                                    Some(map) => map[pixel_idx],
                                    None => 1.0,
                                };
                                if support.contributes() {
                                    let v = match frame_norms {
                                        Some(fnm) => {
                                            let cn = fnm[frame_idx].channels[channel];
                                            chunk[pixel_idx] * cn.gain + cn.offset
                                        }
                                        None => chunk[pixel_idx],
                                    };
                                    values[covered] = v;
                                    eff_weights[covered] = weights
                                        .map_or(1.0, |weights| weights.weight(frame_idx, slot))
                                        * q;
                                    sample_flags[covered] =
                                        flags[frame_idx].map_or(0, |plane| plane[pixel_idx]);
                                    frame_ids[covered] = frame_idx as u32;
                                    if let Some(noise) = noise {
                                        // Confidence is the warp's inverse variance factor.
                                        let model = noise.model(frame_idx, slot);
                                        noise_background[covered] = model.background_variance / q;
                                        noise_sky[covered] = model.sky;
                                        noise_inverse_electrons[covered] = model
                                            .electrons_per_unit
                                            .map_or(0.0, |electrons| 1.0 / (electrons * q));
                                    }
                                    covered += 1;
                                }
                            }
                            let kept = if any_flags {
                                GatheredSamples {
                                    values: &mut *values,
                                    eff_weights: &mut *eff_weights,
                                    sample_flags: &mut *sample_flags,
                                    frame_ids: &mut *frame_ids,
                                    noise_background: &mut *noise_background,
                                    noise_sky: &mut *noise_sky,
                                    noise_inverse_electrons: &mut *noise_inverse_electrons,
                                }
                                .leave_out_flagged(
                                    covered,
                                    min_survivors,
                                    &mut row_excluded,
                                    &mut row_kept,
                                )
                            } else {
                                covered
                            };
                            let sample = if kept == 0 {
                                CombinedSample::uncovered()
                            } else {
                                debug_assert!(
                                    values[..kept].iter().all(|v| v.is_finite()),
                                    "non-finite pixel value entered the combine",
                                );
                                combine(
                                    PixelSamples {
                                        values: &mut values[..kept],
                                        weights: &eff_weights[..kept],
                                        frame_ids: &frame_ids[..kept],
                                        noise: noise.map(|_| NoiseColumns {
                                            background: &noise_background[..kept],
                                            sky: &noise_sky[..kept],
                                            inverse_electrons: &noise_inverse_electrons[..kept],
                                        }),
                                        channel,
                                    },
                                    buffers,
                                )
                            };
                            if let Some(row_flags) = row.flags.as_deref_mut() {
                                let pixel_flags = if covered == 0 {
                                    QualityFlags::NO_DATA
                                } else if sample_flags[..kept].iter().any(|&byte| {
                                    QualityFlags::from_byte(byte)
                                        .intersects(QualityFlags::SATURATED)
                                }) {
                                    QualityFlags::SATURATED
                                } else {
                                    QualityFlags::default()
                                };
                                row_flags[pixel_in_row] |= pixel_flags.byte();
                            }
                            row.value[pixel_in_row] = sample.value;
                            if let Some(weight) = row.weight.as_deref_mut() {
                                weight[pixel_in_row] = sample.weight;
                            }
                            if let Some(variance) = row.variance.as_deref_mut() {
                                variance[pixel_in_row] = sample.variance;
                            }
                            if let Some(dispersion) = row.dispersion.as_deref_mut() {
                                dispersion[pixel_in_row] = sample.dispersion;
                            }
                        }
                        excluded.add(&row_excluded);
                        kept_flagged.add(&row_kept);
                    },
                );
            },
        );
        CombineOutput {
            pixels,
            weight: output_weight,
            variance: output_variance,
            dispersion: output_dispersion,
            flags: output_flags,
            report: RunReport {
                excluded_samples: excluded.totals(),
                kept_flagged_samples: kept_flagged.totals(),
                variance_background_only: planes.variance
                    && noise.is_some_and(|noise| !noise.every_gain_known()),
                spilled_frames: if self.core.tier.spills() {
                    self.frames.len() as u64
                } else {
                    0
                },
                ..RunReport::default()
            },
        }
    }

    /// Each frame's slice of one frame-quality plane over `[start, end)`, `None` where the frame
    /// carries no such plane.
    ///
    /// `plane` picks which of the two a frame's slot is — the combine and the coverage pass both
    /// gather them the same way and differ only in that choice.
    fn quality_chunks(
        &self,
        plane: fn(&StoredFrame) -> Option<&StoredPlane>,
        start: usize,
        end: usize,
    ) -> Vec<Option<&[f32]>> {
        self.frames
            .iter()
            .map(|frame| plane(frame).map(|plane| plane.chunk(start, end)))
            .collect()
    }

    /// What the combine pass holds: one input plane per frame channel, plus one more for each of
    /// that frame's coverage and confidence planes and a byte for its flags, against the resident
    /// output planes.
    fn weighted_layout(&self, planes: QualityPlanes) -> ChunkMemoryLayout {
        ChunkMemoryLayout {
            input_bytes: self
                .frames
                .iter()
                .map(|frame| {
                    (1 + frame.quality.count()) * size_of::<f32>()
                        + usize::from(frame.flags.is_some())
                })
                .sum(),
            resident_planes: self.core.dimensions.channels() * planes.resident_planes_per_channel(),
        }
    }

    /// What the coverage pass holds: one input plane per frame that carries frame quality, against
    /// the combine's residents — which are all still alive at that point — plus the single
    /// coverage plane being accumulated.
    fn coverage_layout(&self, planes: QualityPlanes) -> ChunkMemoryLayout {
        ChunkMemoryLayout {
            input_bytes: self
                .frames
                .iter()
                .filter(|frame| !frame.quality.is_none())
                .count()
                * size_of::<f32>(),
            resident_planes: self.core.dimensions.channels() * planes.resident_planes_per_channel()
                + 1,
        }
    }

    /// Build a cache from CFA calibration frame files, tiered in RAM or on disk under `run`.
    pub(crate) fn from_cfa_paths<P: AsRef<Path> + Sync>(
        paths: &[P],
        config: &StackConfig,
        run: IngestRun,
        step: Option<&dyn FrameStep<CfaImage>>,
        progress: ProgressCallback,
    ) -> Result<Self, StackError> {
        Self::from_tiered_paths(
            loader::load_tiered::<CfaImage, P>(paths, config, run, step, progress)?,
            config.normalization,
        )
    }

    /// Build a cache from light-frame image files, tiered in RAM or on disk under `run`. Nothing
    /// here was warped, so a frame has full support and unit confidence everywhere unless its
    /// source declared pixels with no measurement.
    pub(crate) fn from_paths<P: AsRef<Path> + Sync>(
        paths: &[P],
        config: &StackConfig,
        run: IngestRun,
        progress: ProgressCallback,
    ) -> Result<Self, StackError> {
        Self::from_tiered_paths(
            loader::load_tiered::<LinearImage, P>(paths, config, run, None, progress)?,
            config.normalization,
        )
    }

    fn from_tiered_paths(
        loaded: LoadedCache,
        normalization: Normalization,
    ) -> Result<Self, StackError> {
        let LoadedCache { frames, core } = loaded;
        // The loader ran each frame's own checks as it decoded, and its facts against frame 0's;
        // the facts a later frame states first are compared here, in order.
        let mut facts = SetFacts::default();
        for (index, frame) in frames.iter().enumerate() {
            facts.admit(index, &frame.source_stats.facts)?;
        }
        let frame_norms =
            FrameNorm::measure(&frames, core.dimensions, normalization, &core.cancel)?;
        Ok(Self {
            frames,
            frame_norms,
            core,
        })
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::combine::cache::FrameCache;
    use crate::combine::cache::core::{CacheCore, CacheTier};
    use crate::combine::config::Normalization;
    use crate::frame_store::frame_quality::FrameQuality;
    use crate::frame_store::frame_stats::FrameStats;
    use crate::frame_store::stackable_image::StackableImage;
    use crate::frame_store::stored_frame::StoredFrame;

    impl FrameCache {
        /// A resident cache over already-decoded frames — the shape `from_paths` builds, without
        /// the file round-trip, through the same validation. Nothing here was warped, so a frame
        /// carries quality planes only when its source declared pixels with no measurement.
        pub(crate) fn from_images<I: StackableImage>(
            images: Vec<I>,
            normalization: Normalization,
        ) -> Self {
            let mut core = CacheCore::plain(CacheTier::Resident, images[0].dimensions());
            core.metadata = images[0].metadata().clone();
            let frames = images
                .into_iter()
                .map(|image| {
                    let source_stats = FrameStats::measure(&image);
                    let quality = FrameQuality::for_unwarped(&image);
                    StoredFrame::from_memory(image, quality, source_stats)
                })
                .collect();
            Self::from_stored_frames(frames, core, normalization)
                .expect("test images must be non-empty, dimension-consistent and coverable")
        }
    }
}

#[cfg(test)]
mod tests;
