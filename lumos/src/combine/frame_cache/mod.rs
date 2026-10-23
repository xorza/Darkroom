//! Chunked combine engine for resident and memory-mapped stacking frames.

pub(crate) mod core;
pub(crate) mod frame_step;
pub(crate) mod frame_weights;
pub(crate) mod loader;
pub(crate) mod normalized_stats;
pub(crate) mod sample;
pub(crate) mod sample_noise;

use std::convert::Infallible;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use common::CancelToken;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::combine::error::StackError;
use crate::combine::frame_cache::core::{CacheCore, ChunkCtx};
use crate::combine::frame_cache::frame_step::{FrameStep, SteppedPaths};
use crate::combine::frame_cache::frame_weights::FrameWeights;
use crate::combine::frame_cache::loader::LoadedCache;
use crate::combine::frame_cache::sample::{CombineScratch, FlagTallies, GatheredSamples};
use crate::combine::frame_cache::sample_noise::SampleNoise;
use crate::combine::frame_read::frame_gate::FrameGate;
use crate::combine::frame_read::slots::Slots;
use crate::combine::normalization::slot_planes::SlotPlanes;
use crate::combine::normalization::{FrameNorm, Normalization};
use crate::combine::reducer::combined_sample::CombinedSample;
use crate::combine::reducer::pixel_samples::{NoiseCols, PixelSamples};
use crate::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::combine::stack::combine_cost::CombineCost;
use crate::combine::stack::planned_combine::PlannedCombine;
use crate::combine::stack_frame::StackFrame;
use crate::error::Cancelled;
use crate::frame::capture_conditions::CaptureConditions;
use crate::frame_store::storage_tier::StorageTier;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_layout::StoredLayout;
use crate::image::cfa::cfa_image::CfaImage;
use crate::image::channel_layout::ChannelLayout;
use crate::image::dark_comparison::DarkComparison;
use crate::image::error::FrameDimensionMismatch;
use crate::image::flag_counts::AtomicFlagCounts;
use crate::image::flat_gain::{GainGrid, GainRows};
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::linear_image::LinearImage;
use crate::image::linear_pixels::LinearPixels;
use crate::image::observation_header::ObservationHeader;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;
use crate::image::quality_map::QualityMap;
use crate::ingest::frame_check::FrameCheck;
use crate::ingest::ingest_run::IngestRun;
use crate::ingest::set_facts::SetFacts;
use crate::io::decoder::cfa_decoder::CfaDecoder;
use crate::io::decoder::linear_decoder::LinearDecoder;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::memory::stage::Stage;
use crate::progress::progress_monitor::ProgressMonitor;
use crate::recycle::recycled::Recycled;
use crate::scratch_pool::ScratchPool;
use crate::stack_product::StackProduct;
use crate::stack_product::coverage::Coverage;
use crate::stack_product::quality_planes::QualityPlanes;
use crate::stack_product::run_report::RunReport;
use crate::stack_product::weight_kind::WeightKind;

/// Channel-shaped result of one combine pass. A plane is `None` when [`QualityPlanes`] did not
/// ask for it.
#[derive(Debug)]
pub(crate) struct CombineOutput {
    pub(super) pixels: LinearPixels,
    /// The share of frames gathered at each pixel, when coverage was asked for and some frame
    /// carries frame quality; every pixel is wholly covered otherwise. One plane, or one per
    /// channel when a frame is gathered by each channel's own rule.
    coverage: Option<LinearPixels>,
    weight: Option<LinearPixels>,
    /// What the frame weights summed in `weight` were.
    weight_kind: WeightKind,
    inverse_variance: Option<LinearPixels>,
    dispersion: Option<LinearPixels>,
    /// The stack's flags, for a frame set where a frame carries flags or is not gathered at every
    /// pixel — see [`FrameCache::flag_plane`]: [`QualityFlags::NO_DATA`] where no frame reached a
    /// pixel, [`QualityFlags::SATURATED`] where a kept sample was.
    flags: Option<Buffer2<u8>>,
    report: RunReport,
}

/// The output rows one combine row-task writes: the combined value, plus whichever ancillary
/// planes were requested. Bundling them keeps one gather loop instead of one per plane subset.
#[derive(Debug)]
struct QualityRows<'a> {
    value: &'a mut [f32],
    coverage: Option<&'a mut [f32]>,
    weight: Option<&'a mut [f32]>,
    inverse_variance: Option<&'a mut [f32]>,
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
}

/// The frames feeding one combine, with their normalization parameters. Calibration masters and
/// registered light stacks share it; what separates them is only whether their frames carry quality
/// planes, and even an unwarped one does when its source declared pixels with no measurement.
#[derive(Debug)]
pub(crate) struct FrameCache {
    pub(crate) frames: Vec<StoredFrame>,
    /// How the frames' per-slot figures index a pixel: by channel, or by color of a mosaic.
    pub(crate) slots: Slots,
    /// Each frame's affine onto the reference, measured once at construction for the
    /// normalization the cache was built with; `None` when every frame is combined as it stands.
    pub(crate) frame_norms: Option<Vec<FrameNorm>>,
    pub(crate) core: CacheCore,
}

impl FrameCache {
    /// Build a cache from frames already placed in the shared frame store.
    pub(super) fn from_stored_frames(
        frames: Vec<StoredFrame>,
        core: CacheCore,
        normalization: Normalization,
    ) -> Result<Self, StackError> {
        // The pipeline produced these frames: their geometry, samples and quality pair are its own
        // contracts, checked in debug builds only — on the spill tier a release check would fault
        // every plane in from disk once before the combine reads it again. What the frames' sources
        // stated (domain, row order, pattern) is the input's, and is checked here always.
        Cancelled::check(&core.monitor.cancel)?;
        SetFacts::check_set(frames.iter().map(|frame| &frame.facts))?;
        for (idx, frame) in frames.iter().enumerate() {
            debug_assert!(
                FrameCheck {
                    idx,
                    cancel: &CancelToken::never(),
                }
                .stored(frame, core.dimensions)
                .is_ok(),
                "stored frame {idx} breaks the pipeline's own frame contract"
            );
        }
        Self::measured(frames, core, normalization)
    }

    /// Build an in-memory frame-quality-aware cache from [`StackFrame`]s.
    pub(super) fn from_stack_frames(
        frames: Vec<StackFrame>,
        normalization: Normalization,
        monitor: ProgressMonitor,
    ) -> Result<Self, StackError> {
        debug_assert!(!frames.is_empty(), "`combine_cached` refuses an empty set");
        Cancelled::check(&monitor.cancel)?;
        let dimensions = frames[0].image.dimensions();
        let metadata = frames[0].image.metadata.clone();

        // Width and height before the shared check, which a stored plane can only compare by
        // sample count: a 4×2 frame in a 2×4 set has the right count and the wrong shape.
        for (idx, frame) in frames.iter().enumerate() {
            FrameDimensionMismatch::check(idx, dimensions, frame.image.dimensions())?;
            for (kind, plane) in frame.quality.present() {
                let actual = Size2us::from(plane);
                if actual != dimensions.size() {
                    return Err(StackError::PlaneDimensionMismatch {
                        idx,
                        plane: kind,
                        expected: dimensions.size(),
                        actual,
                    });
                }
            }
        }
        let stored = frames
            .into_iter()
            .map(|frame| StoredFrame::from_memory(frame.image, frame.quality, frame.admitted))
            .collect::<Vec<_>>();
        SetFacts::check_set(stored.iter().map(|frame| &frame.facts))?;
        for (idx, frame) in stored.iter().enumerate() {
            FrameCheck {
                idx,
                cancel: &monitor.cancel,
            }
            .stored(frame, dimensions)?;
        }
        Self::measured(
            stored,
            CacheCore {
                tier: StorageTier::UNPLANNED,
                dimensions,
                metadata,
                monitor,
            },
            normalization,
        )
    }

    /// A cache over `frames`, every check passed, on `core`: their slots, and their norms under
    /// `normalization`.
    fn measured(
        frames: Vec<StoredFrame>,
        core: CacheCore,
        normalization: Normalization,
    ) -> Result<Self, StackError> {
        let slots = Slots::of_frames(&frames, core.dimensions);
        let frame_norms = FrameNorm::measure(
            SlotPlanes {
                frames: &frames,
                dimensions: core.dimensions,
                slots,
                cancel: &core.monitor.cancel,
            },
            normalization,
        )?;
        Ok(Self {
            frames,
            slots,
            frame_norms,
            core,
        })
    }

    /// Assemble the combined image, geometric coverage, and per-channel survivor quality.
    pub(super) fn finish_product(
        &self,
        combined: CombineOutput,
        planes: QualityPlanes,
        quantization_sigma: Option<f32>,
    ) -> StackProduct {
        let CombineOutput {
            pixels,
            coverage,
            weight: weight_pixels,
            weight_kind,
            inverse_variance: inverse_variance_pixels,
            dispersion: dispersion_pixels,
            flags,
            report,
        } = combined;
        let dimensions = self.core.dimensions;
        // Every frame carries the first one's pattern: `SetFacts` held them to it.
        let cfa_type = self.frames[0].facts.cfa_type;
        // Every saturated sample of the stack is flagged only when every frame's was.
        let saturation_flagged = self
            .frames
            .iter()
            .all(|frame| frame.facts.saturation_flagged);
        let conditions =
            CaptureConditions::shared(self.frames.iter().map(|frame| frame.facts.conditions));
        let dark_comparison = self
            .frames
            .iter()
            .filter_map(|frame| frame.facts.dark_comparison)
            .reduce(DarkComparison::union);
        let image = LinearImage::new(
            pixels,
            flags.and_then(PixelFlags::from_bytes),
            // The reference frame's metadata, with the combine's own quantization σ, saturation
            // record, capture conditions and dark comparison, and no mosaic noise: what the
            // reference's decoder and demosaic recorded describes one frame, not the stack. A
            // master carries no flat gain.
            ImageMetadata {
                header: ObservationHeader {
                    exposure_time: conditions.exposure_time,
                    ccd_temp: conditions.ccd_temp,
                    camera_temp: conditions.camera_temp,
                    ..self.core.metadata.header.clone()
                },
                derived: self
                    .core
                    .metadata
                    .derived
                    .clone()
                    .rebuild()
                    .quantization_sigma(quantization_sigma)
                    .mosaic_noise(None)
                    .saturation_flagged(saturation_flagged)
                    .dark_comparison(dark_comparison)
                    .build(),
            },
        );
        StackProduct {
            image,
            coverage: planes.coverage.then(|| match coverage {
                Some(coverage) => Coverage::PerPixel(QualityMap::from(coverage)),
                None => Coverage::Uniform {
                    value: 1.0,
                    size: dimensions.size(),
                },
            }),
            weight: weight_pixels.map(QualityMap::from),
            weight_kind,
            inverse_variance: inverse_variance_pixels.map(QualityMap::from),
            dispersion: dispersion_pixels.map(QualityMap::from),
            cfa_type,
            report,
        }
    }

    /// The combine: for each output pixel, gather the frames that cover it, hand them to
    /// `combine`, and write the reduced value plus whichever [`QualityPlanes`] were requested.
    ///
    /// [`FrameGate`] alone decides whether a frame is gathered at a pixel; the sample carries its
    /// frame's weight into the reduction, and its confidence, guaranteed positive wherever the
    /// frame was gathered, divides its noise model. A frame carrying no frame quality contributes
    /// everywhere at unit confidence, which is what lets calibration masters and registered light
    /// stacks share this loop. A pixel no frame supports gets `0`.
    pub(super) fn process_chunked<Combine>(
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
        } = request;
        let slots = self.slots;
        debug_assert!(noise.is_none_or(|noise| noise.slots() == slots));
        // An in-memory stack is one chunk, so the per-chunk cancel check in
        // `process_chunks` can't interrupt the combine — poll per row here too.
        let cancel = self.core.monitor.cancel.clone();
        let frame_norms = self.frame_norms.as_deref();
        let dimensions = self.core.dimensions;
        let chunking = self.core.chunk_rows(self.combine_stage(planes));
        let mut output_weight = planes.weight.then(|| LinearPixels::new_zeroed(dimensions));
        let mut output_inverse_variance = planes
            .inverse_variance
            .then(|| LinearPixels::new_zeroed(dimensions));
        let mut output_dispersion = planes
            .dispersion
            .then(|| LinearPixels::new_zeroed(dimensions));
        let mut output_coverage = self.coverage_layout(planes).map(|layout| {
            LinearPixels::new_zeroed(ImageDimensions::new(dimensions.size(), layout))
        });
        let inverse_frame_count = 1.0 / self.frames.len() as f32;
        let any_flags = self.frames.iter().any(|frame| frame.flags.is_some());
        let mut output_flags = self
            .flag_plane()
            .then(|| Buffer2::<u8>::new_default(dimensions.width(), dimensions.height()));
        let excluded = AtomicFlagCounts::default();
        let kept_flagged = AtomicFlagCounts::default();
        let fallback_pixels = AtomicU64::new(0);
        // One pool for the whole combine. `process_chunks` invokes the row loop below once per
        // chunk per channel, so the leases have to outlive any single `for_each_init`.
        let scratch_pool = ScratchPool::<CombineScratch>::new(self.frames.len());
        // What each chunk borrows its planes through, kept empty between chunks so the combine
        // allocates each once.
        let mut kept_gates: Recycled<FrameGate<'static>> = Recycled::default();
        let mut kept_flags: Recycled<Option<&'static [u8]>> = Recycled::default();
        let mut kept_gains: Recycled<Option<GainRows<'static>>> = Recycled::default();
        let mut kept_rows: Recycled<QualityRows<'static>> = Recycled::default();
        let pixels = self
            .core
            .process_chunks(&self.frames, chunking.units, |output_slice, ctx| {
                let ChunkCtx {
                    frames,
                    chunks,
                    width,
                    channel,
                    pixel_offset,
                } = ctx;
                debug_assert_eq!(frames.len(), self.frames.len());
                let chunk_pixels = output_slice.len();
                debug_assert_eq!(pixel_offset % width, 0, "a chunk starts at a row");
                let chunk_end = pixel_offset + chunk_pixels;
                let mut gates: Vec<FrameGate<'_>> = kept_gates.take();
                gates.extend(
                    chunks
                        .iter()
                        .map(|chunk| FrameGate::of_chunk(chunk, channel)),
                );
                let mut flags: Vec<Option<&[u8]>> = kept_flags.take();
                flags.extend(chunks.iter().map(|chunk| chunk.flags));
                // The node rows of each frame's flat gain the chunk reads, for the noise model, and
                // the grid they share: every frame has the stack's size.
                let gain_grid = GainGrid::of(dimensions.size());
                let mut gains: Vec<Option<GainRows<'_>>> = kept_gains.take();
                gains.extend(self.frames.iter().map(|frame| {
                    frame
                        .flat_gain
                        .as_ref()
                        .filter(|_| noise.is_some())
                        .map(|gain| {
                            debug_assert_eq!(gain.grid(), gain_grid);
                            debug_assert_eq!(gain.channels(), slots.count());
                            gain.rows(pixel_offset / width..chunk_end / width)
                        })
                }));
                // One row bundle per output row, so an unrequested plane simply has no slice
                // to write instead of needing its own copy of the gather loop below.
                let mut rows: Vec<QualityRows<'_>> = kept_rows.take();
                rows.extend(output_slice.chunks_mut(width).map(|value| QualityRows {
                    value,
                    coverage: None,
                    weight: None,
                    inverse_variance: None,
                    dispersion: None,
                    flags: None,
                }));
                // A coverage plane shared by the channels is their one gate, so the first channel's
                // gather writes it; one per channel is written by its own.
                let chunk = pixel_offset..pixel_offset + chunk_pixels;
                if let Some(pixels) = output_coverage
                    .as_mut()
                    .filter(|pixels| pixels.layout() == ChannelLayout::Rgb || channel == 0)
                {
                    let plane = channel.min(pixels.channel_count() - 1);
                    let plane = &mut pixels.channel_mut(plane).pixels_mut()[chunk.clone()];
                    attach_rows(&mut rows, plane, width, |row| &mut row.coverage);
                }
                if let Some(pixels) = output_weight.as_mut() {
                    let plane = &mut pixels.channel_mut(channel).pixels_mut()[chunk.clone()];
                    attach_rows(&mut rows, plane, width, |row| &mut row.weight);
                }
                if let Some(pixels) = output_inverse_variance.as_mut() {
                    let plane = &mut pixels.channel_mut(channel).pixels_mut()[chunk.clone()];
                    attach_rows(&mut rows, plane, width, |row| &mut row.inverse_variance);
                }
                if let Some(pixels) = output_dispersion.as_mut() {
                    let plane = &mut pixels.channel_mut(channel).pixels_mut()[chunk.clone()];
                    attach_rows(&mut rows, plane, width, |row| &mut row.dispersion);
                }
                // One plane for every channel, each pass ORing into it: a pixel is flagged when any
                // of its channels is.
                if let Some(plane) = output_flags.as_mut() {
                    let plane = &mut plane.pixels_mut()[chunk];
                    attach_rows(&mut rows, plane, width, |row| &mut row.flags);
                }
                rows.par_drain(..).enumerate().for_each_init(
                    || scratch_pool.acquire(),
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
                            noise_bg,
                            noise_sky,
                            noise_per_unit,
                            buffers,
                        } = &mut **scratch;
                        let values = values.as_mut_slice();
                        let eff_weights = eff_weights.as_mut_slice();
                        let sample_flags = sample_flags.as_mut_slice();
                        let noise_bg = noise_bg.as_mut_slice();
                        let noise_sky = noise_sky.as_mut_slice();
                        let noise_per_unit = noise_per_unit.as_mut_slice();
                        let mut row_tallies = FlagTallies::default();
                        let mut row_fallbacks = 0u64;
                        let row_offset = row_in_chunk * width;
                        let y = pixel_offset / width + row_in_chunk;
                        for pixel_in_row in 0..width {
                            let pixel_idx = row_offset + pixel_in_row;
                            let slot = slots.slot(channel, Vec2us::new(pixel_in_row, y));
                            let gain_point = gain_grid.point(pixel_in_row as f32, y as f32);
                            let mut covered = 0usize;
                            for (frame_idx, chunk) in frames.iter().enumerate() {
                                if let Some(gathered) = gates[frame_idx].sample(pixel_idx) {
                                    let q = gathered.confidence;
                                    let v = match frame_norms {
                                        Some(fnm) => {
                                            let norm = fnm[frame_idx].slots[slot];
                                            chunk[pixel_idx] * norm.gain + norm.offset
                                        }
                                        None => chunk[pixel_idx],
                                    };
                                    values[covered] = v;
                                    // The frame's weight, times a drizzle's drop weight: the
                                    // warp's confidence enters the noise model below, never the
                                    // mean (`WarpResult`).
                                    eff_weights[covered] = weights
                                        .map_or(1.0, |weights| weights.weight(frame_idx, slot))
                                        * gathered.weight;
                                    sample_flags[covered] =
                                        flags[frame_idx].map_or(0, |plane| plane[pixel_idx]);
                                    if let Some(noise) = noise {
                                        // Confidence is the warp's inverse variance factor, the
                                        // flat's gain the factor it multiplied the sample by.
                                        let gain = gains[frame_idx]
                                            .as_ref()
                                            .map_or(1.0, |rows| rows.at(slot, gain_point));
                                        let model = noise.model(frame_idx, slot).at(gain).over(q);
                                        noise_bg[covered] = model.bg;
                                        noise_sky[covered] = model.sky;
                                        noise_per_unit[covered] = model.per_unit;
                                    }
                                    covered += 1;
                                }
                            }
                            let kept = if any_flags {
                                GatheredSamples {
                                    values: &mut *values,
                                    eff_weights: &mut *eff_weights,
                                    sample_flags: &mut *sample_flags,
                                    noise_bg: &mut *noise_bg,
                                    noise_sky: &mut *noise_sky,
                                    noise_per_unit: &mut *noise_per_unit,
                                }
                                .leave_out_flagged(
                                    covered,
                                    min_survivors,
                                    &mut row_tallies,
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
                                        noise: noise.map(|_| NoiseCols {
                                            bg: &noise_bg[..kept],
                                            sky: &noise_sky[..kept],
                                            per_unit: &noise_per_unit[..kept],
                                        }),
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
                            row_fallbacks += u64::from(sample.small_n_fallback);
                            row.value[pixel_in_row] = sample.value;
                            if let Some(coverage) = row.coverage.as_deref_mut() {
                                coverage[pixel_in_row] = covered as f32 * inverse_frame_count;
                            }
                            if let Some(weight) = row.weight.as_deref_mut() {
                                weight[pixel_in_row] = sample.weight;
                            }
                            if let Some(plane) = row.inverse_variance.as_deref_mut() {
                                plane[pixel_in_row] = sample.inverse_variance;
                            }
                            if let Some(dispersion) = row.dispersion.as_deref_mut() {
                                dispersion[pixel_in_row] = sample.dispersion;
                            }
                        }
                        excluded.add(&row_tallies.excluded);
                        kept_flagged.add(&row_tallies.kept);
                        fallback_pixels.fetch_add(row_fallbacks, Ordering::Relaxed);
                    },
                );
                kept_rows.put(rows);
                kept_gains.put(gains);
                kept_flags.put(flags);
                kept_gates.put(gates);
            });
        CombineOutput {
            pixels,
            coverage: output_coverage,
            weight: output_weight,
            weight_kind: weights.map_or(WeightKind::Count, FrameWeights::kind),
            inverse_variance: output_inverse_variance,
            dispersion: output_dispersion,
            flags: output_flags,
            report: RunReport {
                excluded_samples: excluded.totals(),
                kept_flagged_samples: kept_flagged.totals(),
                small_n_fallback_pixels: fallback_pixels.into_inner(),
                variance_bg_only: planes.inverse_variance
                    && noise.is_some_and(|noise| noise.first_without_gain().is_some()),
                overcommit_bytes: chunking.overcommit_bytes,
                planned_peak_bytes: self.core.tier.planned_peak_bytes(),
                spilled_frames: if self.core.tier.spills() {
                    self.frames.len() as u64
                } else {
                    0
                },
                ..RunReport::default()
            },
        }
    }

    /// The coverage planes the combine writes: none unless one was asked for and some frame
    /// carries quality planes, without which every pixel is wholly covered — and saying so costs
    /// one number rather than an image-sized plane of `1.0`; one per channel when a frame is
    /// gathered by each channel's own rule; one otherwise.
    fn coverage_layout(&self, planes: QualityPlanes) -> Option<ChannelLayout> {
        if !planes.coverage || self.frames.iter().all(|frame| frame.quality.is_none()) {
            None
        } else if self.frames.iter().any(|frame| frame.quality.per_channel()) {
            Some(self.core.dimensions.layout())
        } else {
            Some(ChannelLayout::Mono)
        }
    }

    /// Whether the combine writes a flag plane: when a frame carries flags, whose saturation the
    /// stack keeps, or carries frame quality, which can leave a pixel no frame reached.
    fn flag_plane(&self) -> bool {
        self.frames
            .iter()
            .any(|frame| frame.flags.is_some() || !frame.quality.is_none())
    }

    /// The combine as a stage of rows: the [`CombineCost`] of the frames' layouts, the coverage
    /// planes when any are gathered, and the flag plane when the combine writes one.
    fn combine_stage(&self, planes: QualityPlanes) -> Stage {
        CombineCost {
            dimensions: self.core.dimensions,
            quality: planes,
            coverage_planes: self.coverage_layout(planes).map_or(0, ChannelLayout::count),
            flag_planes: usize::from(self.flag_plane()),
            frame_pixel_bytes: self
                .frames
                .iter()
                .map(|frame| StoredLayout::of(frame).pixel_bytes())
                .sum(),
            frame_count: self.frames.len(),
            // The pool the combine leases from holds one scratch for each worker of the pool it
            // runs on.
            workers: rayon::current_num_threads(),
        }
        .stage(0)
    }

    /// Build a cache from CFA calibration frame files, tiered in RAM or on disk under `run`.
    pub(crate) fn from_cfa_paths<P, S, E>(
        frames: SteppedPaths<'_, P, S>,
        combine: PlannedCombine,
        run: IngestRun,
    ) -> Result<Self, E>
    where
        P: AsRef<Path>,
        S: FrameStep<CfaImage>,
        E: From<StackError> + From<S::Error> + From<Cancelled> + Send,
    {
        let paths: Vec<&Path> = frames.paths.iter().map(AsRef::as_ref).collect();
        let frames = SteppedPaths {
            paths: &paths,
            step: frames.step,
        };
        Ok(Self::from_tiered_paths(
            loader::load_tiered::<CfaDecoder, _, E>(frames, combine, run)?,
            combine.normalization,
        )?)
    }

    /// Build a cache from light-frame image files, tiered in RAM or on disk under `run`. Nothing
    /// here was warped, so a frame has full support and unit confidence everywhere unless its
    /// source declared pixels with no measurement.
    pub(crate) fn from_paths(
        paths: &[&Path],
        combine: PlannedCombine,
        run: IngestRun,
    ) -> Result<Self, StackError> {
        Self::from_tiered_paths(
            loader::load_tiered::<LinearDecoder, _, StackError>(
                SteppedPaths {
                    paths,
                    step: &None::<Infallible>,
                },
                combine,
                run,
            )?,
            combine.normalization,
        )
    }

    fn from_tiered_paths(
        loaded: LoadedCache,
        normalization: Normalization,
    ) -> Result<Self, StackError> {
        let LoadedCache { frames, core } = loaded;
        // The loader ran each frame's own checks as it decoded, and its facts against frame 0's;
        // the facts a later frame states first are compared here, in order.
        SetFacts::check_set(frames.iter().map(|frame| &frame.facts))?;
        Self::measured(frames, core, normalization)
    }
}

/// Hand each of `rows` its row of `plane`, a chunk of rows `width` wide, through the slot `field`
/// names.
fn attach_rows<'a, T>(
    rows: &mut [QualityRows<'a>],
    plane: &'a mut [T],
    width: usize,
    field: impl for<'r> Fn(&'r mut QualityRows<'a>) -> &'r mut Option<&'a mut [T]>,
) {
    for (row, chunk) in rows.iter_mut().zip(plane.chunks_mut(width)) {
        *field(row) = Some(chunk);
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::combine::frame_cache::FrameCache;
    use crate::combine::frame_cache::core::CacheCore;
    use crate::combine::normalization::Normalization;
    use crate::frame::admitted::Admitted;
    use crate::frame::frame_quality::FrameQuality;
    use crate::frame_store::storage_tier::StorageTier;
    use crate::frame_store::stored_frame::StoredFrame;
    use crate::image::stackable_image::StackableImage;

    impl FrameCache {
        /// A resident cache over already-decoded frames — the shape `from_paths` builds, without
        /// the file round-trip, through the same validation. Nothing here was warped, so a frame
        /// carries quality planes only when its source declared pixels with no measurement.
        pub(crate) fn from_images<I: StackableImage>(
            images: Vec<I>,
            normalization: Normalization,
        ) -> Self {
            let mut core = CacheCore::plain(StorageTier::UNPLANNED, images[0].dimensions());
            core.metadata = images[0].metadata().clone();
            let frames = images
                .into_iter()
                .map(|image| {
                    let admitted = Admitted::measure(&image);
                    let quality = FrameQuality::for_unwarped(image.flag_counts());
                    StoredFrame::from_memory(image, quality, admitted)
                })
                .collect();
            Self::from_stored_frames(frames, core, normalization)
                .expect("test images must be non-empty, dimension-consistent and coverable")
        }
    }
}

#[cfg(test)]
mod tests;
