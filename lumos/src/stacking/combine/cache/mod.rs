//! Chunked combine engine for resident and memory-mapped stacking frames.

mod loader;

use common::CancelToken;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::linear::{LinearImage, PixelData};
use crate::io::image::{ImageDimensions, ImageMetadata};
use crate::stacking::combine::MIN_CONTRIBUTING_COVERAGE;
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::normalization::{FrameNorm, compute_light_frame_norms};
use crate::stacking::combine::stack::StackFrame;
use crate::stacking::frame_store::{
    ChunkMemoryLayout, FrameStats, SpillDirectory, StackableImage, StoredFrame, StoredLightFrame,
    StoredPlane, optimal_chunk_rows,
};
use crate::stacking::product::{QualityMap, StackProduct};
use crate::stacking::progress::{ProgressCallback, StackingStage, report_progress};

/// Per-thread scratch buffers for stacking combine closures.
///
/// Allocated once per rayon thread via `for_each_init` and reused across all pixels.
#[derive(Debug, Default)]
pub(crate) struct ScratchBuffers {
    /// Tracks original frame indices after rejection reordering.
    pub(crate) indices: Vec<usize>,
    /// General-purpose f32 scratch (e.g. winsorized working copy).
    pub(crate) floats_a: Vec<f32>,
    /// Second f32 scratch (e.g. median/MAD computation).
    pub(crate) floats_b: Vec<f32>,
    /// usize scratch (large-N `sort_with_indices` permutation).
    pub(crate) usize_a: Vec<usize>,
    /// Second usize scratch (large-N `sort_with_indices` index copy).
    pub(crate) usize_b: Vec<usize>,
    pub(crate) gesd_statistics: Vec<f64>,
    pub(crate) gesd_critical_values: Vec<f64>,
    pub(crate) gesd_sample_count: usize,
    pub(crate) gesd_alpha_bits: u32,
}

impl ScratchBuffers {
    fn new(frame_count: usize) -> Self {
        Self {
            indices: Vec::with_capacity(frame_count),
            floats_a: Vec::with_capacity(frame_count),
            floats_b: Vec::with_capacity(frame_count),
            usize_a: Vec::with_capacity(frame_count),
            usize_b: Vec::with_capacity(frame_count),
            gesd_statistics: Vec::with_capacity(frame_count / 4),
            gesd_critical_values: Vec::with_capacity(frame_count / 4),
            gesd_sample_count: 0,
            gesd_alpha_bits: 0,
        }
    }
}

/// One reduced channel sample and the effective quality of the samples that survived rejection.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CombinedSample {
    pub(crate) value: f32,
    pub(crate) weight: f32,
    pub(crate) linear_variance: f32,
}

impl CombinedSample {
    pub(crate) fn from_all(value: f32, weights: &[f32]) -> Self {
        Self::from_survivors(value, weights, 0..weights.len())
    }

    pub(crate) fn from_survivors(
        value: f32,
        weights: &[f32],
        survivor_indices: impl IntoIterator<Item = usize>,
    ) -> Self {
        let mut weight = 0.0f32;
        let mut weight_squared = 0.0f32;
        for index in survivor_indices {
            let survivor_weight = weights[index];
            weight += survivor_weight;
            weight_squared += survivor_weight * survivor_weight;
        }
        let linear_variance = if weight > 0.0 {
            weight_squared / (weight * weight)
        } else {
            0.0
        };
        Self {
            value,
            weight,
            linear_variance,
        }
    }
}

/// Channel-shaped result of one light-frame combine pass and its pre-output memory snapshot.
#[derive(Debug)]
pub(crate) struct LightCombineOutput {
    pub(crate) pixels: PixelData,
    pub(crate) weight: PixelData,
    pub(crate) linear_variance: Option<PixelData>,
    chunk_available_memory: Option<u64>,
}

/// Shared cache context + combine engine — everything that doesn't depend on the frame type.
/// Owned by composition inside [`CfaCache`] and [`LightCache`]; all frames share one tier, and
/// `spill_directory` is `Some` only when the planes are memory-mapped.
#[derive(Debug)]
pub(crate) struct CacheCore {
    pub(crate) spill_directory: Option<SpillDirectory>,
    /// Image dimensions (same for all frames).
    pub(crate) dimensions: ImageDimensions,
    /// Metadata from the first frame.
    pub(crate) metadata: ImageMetadata,
    /// Configuration for cache operations.
    pub(crate) config: CacheConfig,
    /// Progress callback.
    pub(crate) progress: ProgressCallback,
    /// Cooperative cancel flag, present during validation and normalization and polled by
    /// [`Self::process_chunks`] during the combine.
    pub(crate) cancel: CancelToken,
}

/// Plain calibration cache: `CfaImage` frames, channels only (never coverage), plain combine.
/// Built from disk via [`CfaCache::from_paths`]; stacks to a `CfaImage`.
#[derive(Debug)]
pub(crate) struct CfaCache {
    // Stored planes drop before the spill directory owner in `core`.
    pub(crate) frames: Vec<StoredFrame>,
    pub(crate) frame_stats: Vec<FrameStats>,
    pub(crate) core: CacheCore,
}

/// Light-frame stacking cache with optional support and interpolation-confidence planes.
#[derive(Debug)]
pub(crate) struct LightCache {
    pub(crate) frames: Vec<StoredLightFrame>,
    pub(crate) frame_norms: Option<Vec<FrameNorm>>,
    pub(crate) core: CacheCore,
}

#[derive(Debug)]
pub(crate) struct StoredLightCacheParams {
    pub(crate) spill_directory: Option<SpillDirectory>,
    pub(crate) dimensions: ImageDimensions,
    pub(crate) metadata: ImageMetadata,
    pub(crate) config: CacheConfig,
    pub(crate) normalization: Normalization,
    pub(crate) progress: ProgressCallback,
    pub(crate) cancel: CancelToken,
}

/// Per-chunk context handed to the [`CacheCore::process_chunks`] closure: the input frame
/// slices for this chunk plus the geometry to map a within-chunk pixel to a global frame index.
#[derive(Debug)]
struct ChunkContext<'a> {
    /// One channel slice per frame for this chunk; `frames.len()` is the frame count.
    frames: &'a [&'a [f32]],
    /// Row width in pixels.
    width: usize,
    /// Channel currently being combined.
    channel: usize,
    /// Global pixel index of this chunk's first pixel — for indexing full-frame,
    /// channel-independent maps such as coverage.
    pixel_offset: usize,
}

const VALIDATION_CHUNK_SIZE: usize = 16_384;

fn check_cancel(cancel: &CancelToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(())
}

fn validate_sample_channels<'a>(
    index: usize,
    channels: impl IntoIterator<Item = &'a [f32]>,
    cancel: &CancelToken,
) -> Result<(), Error> {
    for (channel, samples) in channels.into_iter().enumerate() {
        for (pixel, value) in samples.iter().copied().enumerate() {
            if pixel.is_multiple_of(VALIDATION_CHUNK_SIZE) {
                check_cancel(cancel)?;
            }
            if !value.is_finite() {
                return Err(Error::NonFiniteImageSample {
                    index,
                    channel,
                    pixel,
                    value,
                });
            }
        }
    }
    Ok(())
}

fn validate_image_samples(
    image: &impl StackableImage,
    index: usize,
    cancel: &CancelToken,
) -> Result<(), Error> {
    validate_sample_channels(
        index,
        (0..image.dimensions().channels()).map(|channel| image.channel(channel)),
        cancel,
    )
}

fn validate_stored_samples(
    channels: &[StoredPlane],
    pixel_count: usize,
    index: usize,
    cancel: &CancelToken,
) -> Result<(), Error> {
    validate_sample_channels(
        index,
        channels.iter().map(|plane| plane.chunk(0, pixel_count)),
        cancel,
    )
}

fn validate_warp_plane_values(
    index: usize,
    plane_name: &'static str,
    samples: &[f32],
    cancel: &CancelToken,
) -> Result<(), Error> {
    for (pixel, value) in samples.iter().copied().enumerate() {
        if pixel.is_multiple_of(VALIDATION_CHUNK_SIZE) {
            check_cancel(cancel)?;
        }
        let invalid = !value.is_finite()
            || if plane_name == "coverage" {
                !(0.0..=1.0).contains(&value)
            } else {
                value < 0.0
            };
        if invalid {
            return Err(Error::InvalidWarpPlaneValue {
                index,
                plane: plane_name,
                pixel,
                value,
            });
        }
    }
    Ok(())
}

fn weighted_chunk_memory_layout(
    frames: &[StoredLightFrame],
    output_channels: usize,
) -> ChunkMemoryLayout {
    ChunkMemoryLayout {
        input_planes: frames
            .iter()
            .map(|frame| {
                1 + usize::from(frame.coverage.is_some()) + usize::from(frame.confidence.is_some())
            })
            .sum(),
        resident_planes: 3 * output_channels,
    }
}

impl CacheCore {
    fn chunk_available_memory(&self) -> Option<u64> {
        self.spill_directory
            .as_ref()
            .map(|_| self.config.get_available_memory())
    }

    /// Combine engine: walk the output in memory-bounded row chunks (whole planes for in-memory
    /// stacks, bounded row chunks for disk-backed), gather each frame's channel slice for the
    /// chunk via [`StoredPlane::chunk`], and hand `(output_slice, ChunkContext)` to `process`. The frames
    /// live in the owning cache, so they're passed in. Returns the combined `PixelData`.
    fn process_chunks<F, Channels, Process>(
        &self,
        frames: &[F],
        frame_channels: Channels,
        memory: ChunkMemoryLayout,
        available_memory: Option<u64>,
        mut process: Process,
    ) -> PixelData
    where
        Channels: for<'a> Fn(&'a F) -> &'a [StoredPlane] + Copy,
        Process: FnMut(&mut [f32], ChunkContext),
    {
        let dims = self.dimensions;
        let frame_count = frames.len();
        let width = dims.width();
        let height = dims.height();

        let chunk_rows = available_memory.map_or(height, |available_memory| {
            optimal_chunk_rows(width, height, memory, available_memory)
        });

        let mut output = PixelData::new_default(width, height, dims.channels());
        let channel_count = output.channels();

        let num_chunks = height.div_ceil(chunk_rows);
        let total_work = num_chunks * channel_count;

        let mut chunks: Vec<&[f32]> = Vec::with_capacity(frame_count);

        report_progress(&self.progress, 0, total_work, StackingStage::Processing);

        for channel in 0..channel_count {
            for chunk_idx in 0..num_chunks {
                let start_row = chunk_idx * chunk_rows;
                let end_row = (start_row + chunk_rows).min(height);
                let rows_in_chunk = end_row - start_row;
                let pixels_in_chunk = rows_in_chunk * width;

                chunks.clear();
                chunks.extend((0..frame_count).map(|frame_idx| {
                    self.read_channel_chunk(
                        frames,
                        frame_channels,
                        frame_idx,
                        channel,
                        start_row,
                        end_row,
                    )
                }));

                let output_slice = &mut output.channel_mut(channel).pixels_mut()
                    [start_row * width..][..pixels_in_chunk];

                process(
                    output_slice,
                    ChunkContext {
                        frames: &chunks,
                        width,
                        channel,
                        pixel_offset: start_row * width,
                    },
                );

                report_progress(
                    &self.progress,
                    channel * num_chunks + chunk_idx + 1,
                    total_work,
                    StackingStage::Processing,
                );

                // Cooperative cancel: bail between chunks (the in-flight chunk
                // completes). The partial `output` is discarded by the caller,
                // which detects the cancel and returns `Error::Cancelled`.
                if self.cancel.is_cancelled() {
                    return output;
                }
            }
        }

        output
    }

    /// Read a horizontal chunk (rows `start_row..end_row`) of a single channel from one frame,
    /// tier-agnostically via [`StoredPlane::chunk`].
    fn read_channel_chunk<'a, F, Channels>(
        &self,
        frames: &'a [F],
        channels: Channels,
        frame_idx: usize,
        channel: usize,
        start_row: usize,
        end_row: usize,
    ) -> &'a [f32]
    where
        Channels: Fn(&'a F) -> &'a [StoredPlane],
    {
        let width = self.dimensions.width();
        channels(&frames[frame_idx])[channel].chunk(start_row * width, end_row * width)
    }
}

impl CfaCache {
    /// Plain per-pixel combine: every frame contributes at every pixel (CFA frames have no
    /// coverage). Optional `weights` provide per-frame weights; `frame_norms` apply per-frame
    /// affine normalization before combining. Per-channel, parallelized per-row with rayon.
    pub(crate) fn process_chunked<Combine>(
        &self,
        weights: Option<&[f32]>,
        frame_norms: Option<&[FrameNorm]>,
        combine: Combine,
    ) -> PixelData
    where
        Combine: Fn(&mut [f32], Option<&[f32]>, &mut ScratchBuffers) -> f32 + Sync,
    {
        if let Some(w) = weights {
            assert_eq!(
                w.len(),
                self.frames.len(),
                "Weight count must match frame count"
            );
        }
        // An in-memory stack is one chunk, so the per-chunk cancel check in
        // `process_chunks` can't interrupt the combine — poll per row here too.
        let cancel = self.core.cancel.clone();
        let chunk_available_memory = self.core.chunk_available_memory();
        self.core.process_chunks(
            &self.frames,
            |frame| &frame.channels,
            ChunkMemoryLayout {
                input_planes: self.frames.len(),
                resident_planes: self.core.dimensions.channels(),
            },
            chunk_available_memory,
            |output_slice, ctx| {
                let ChunkContext {
                    frames,
                    width,
                    channel,
                    pixel_offset: _,
                } = ctx;
                let frame_count = frames.len();
                output_slice
                    .par_chunks_mut(width)
                    .enumerate()
                    .for_each_init(
                        || (vec![0.0f32; frame_count], ScratchBuffers::new(frame_count)),
                        |(values, scratch), (row_in_chunk, row_output)| {
                            // Cancelled: skip the row's work (output stays zero; the
                            // caller discards the partial result and reports Cancelled).
                            if cancel.is_cancelled() {
                                return;
                            }
                            let row_offset = row_in_chunk * width;
                            for (pixel_in_row, out) in row_output.iter_mut().enumerate() {
                                let pixel_idx = row_offset + pixel_in_row;
                                if let Some(frame_norm) = frame_norms {
                                    for (frame_idx, chunk) in frames.iter().enumerate() {
                                        let cn = frame_norm[frame_idx].channels[channel];
                                        values[frame_idx] = chunk[pixel_idx] * cn.gain + cn.offset;
                                    }
                                } else {
                                    for (frame_idx, chunk) in frames.iter().enumerate() {
                                        values[frame_idx] = chunk[pixel_idx];
                                    }
                                }
                                // The combine reducers (median/MAD, precise sums) assume finite
                                // inputs — NaN/Inf would silently corrupt the output (NaN-unsafe
                                // ordering, multiply-through). No production path emits them today
                                // (FITS load rejects them, flat division is floored, warp border-fills 0),
                                // so this guards against a future upstream regression.
                                debug_assert!(
                                    values.iter().all(|v| v.is_finite()),
                                    "non-finite pixel value entered the combine",
                                );
                                *out = combine(values, weights, scratch);
                            }
                        },
                    );
            },
        )
    }
}

impl LightCache {
    /// Build a cache from frames already placed in the shared frame store.
    pub(crate) fn from_stored_frames(
        frames: Vec<StoredLightFrame>,
        params: StoredLightCacheParams,
    ) -> Result<Self, Error> {
        let StoredLightCacheParams {
            spill_directory,
            dimensions,
            metadata,
            config,
            normalization,
            progress,
            cancel,
        } = params;
        check_cancel(&cancel)?;
        for (index, frame) in frames.iter().enumerate() {
            validate_stored_samples(&frame.channels, dimensions.pixel_count(), index, &cancel)?;
        }
        let frame_norms = compute_light_frame_norms(&frames, dimensions, normalization, &cancel)?;
        Ok(Self {
            frames,
            frame_norms,
            core: CacheCore {
                spill_directory,
                dimensions,
                metadata,
                config,
                progress,
                cancel,
            },
        })
    }

    /// Build an in-memory warp-quality-aware cache from [`StackFrame`]s.
    pub(crate) fn from_stack_frames(
        frames: Vec<StackFrame>,
        config: &CacheConfig,
        normalization: Normalization,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<Self, Error> {
        if frames.is_empty() {
            return Err(Error::NoFrames);
        }
        check_cancel(&cancel)?;
        let dimensions = frames[0].image.dimensions();
        let metadata = frames[0].image.metadata.clone();

        for (index, frame) in frames.iter().enumerate() {
            check_cancel(&cancel)?;
            if index > 0 && frame.image.dimensions() != dimensions {
                return Err(Error::DimensionMismatch {
                    index,
                    expected: dimensions,
                    actual: frame.image.dimensions(),
                });
            }
            validate_image_samples(&frame.image, index, &cancel)?;
            for (plane_name, plane) in [
                ("coverage", frame.coverage.as_ref()),
                ("confidence", frame.confidence.as_ref()),
            ] {
                if let Some(plane) = plane
                    && (plane.width(), plane.height()) != (dimensions.width(), dimensions.height())
                {
                    return Err(Error::WarpPlaneDimensionMismatch {
                        index,
                        plane: plane_name,
                        expected_width: dimensions.width(),
                        expected_height: dimensions.height(),
                        actual_width: plane.width(),
                        actual_height: plane.height(),
                    });
                }
                if let Some(plane) = plane {
                    validate_warp_plane_values(index, plane_name, plane.pixels(), &cancel)?;
                }
            }
        }
        check_cancel(&cancel)?;
        let stored = frames
            .into_iter()
            .map(|frame| {
                StoredLightFrame::from_memory(
                    frame.image,
                    frame.coverage,
                    frame.confidence,
                    frame.source_stats,
                )
            })
            .collect::<Vec<_>>();
        let frame_norms = compute_light_frame_norms(&stored, dimensions, normalization, &cancel)?;

        Ok(Self {
            frames: stored,
            frame_norms,
            core: CacheCore {
                spill_directory: None,
                dimensions,
                metadata,
                config: config.clone(),
                progress,
                cancel,
            },
        })
    }

    /// Assemble the combined image, geometric coverage, and per-channel survivor quality.
    pub(crate) fn finish_product(&self, combined: LightCombineOutput) -> StackProduct {
        let LightCombineOutput {
            pixels,
            weight: weight_pixels,
            linear_variance: linear_variance_pixels,
            chunk_available_memory,
        } = combined;
        let dimensions = self.core.dimensions;
        let image = LinearImage {
            metadata: self.core.metadata.clone(),
            dimensions,
            pixels,
        };
        let weight = QualityMap::from_pixels(weight_pixels);
        let linear_variance = linear_variance_pixels.map(QualityMap::from_pixels);
        let frame_count = self.frames.len();
        let width = dimensions.width();
        let height = dimensions.height();

        if self.frames.iter().all(|frame| frame.coverage.is_none()) {
            return StackProduct {
                image,
                coverage: Buffer2::new_filled(width, height, 1.0),
                weight,
                linear_variance,
            };
        }

        let mut coverage = Buffer2::new_default(width, height);
        let inv_frames = 1.0 / frame_count as f32;

        // Coverage planes share their frame's tier, so they may be mmap-backed: read them in the
        // same row-aligned chunks the combine uses.
        let chunk_rows = chunk_available_memory.map_or(height, |available_memory| {
            let input_planes = self
                .frames
                .iter()
                .filter(|frame| frame.coverage.is_some())
                .count();
            let resident_planes =
                dimensions.channels() * (2 + usize::from(linear_variance.is_some())) + 1;
            optimal_chunk_rows(
                width,
                height,
                ChunkMemoryLayout {
                    input_planes,
                    resident_planes,
                },
                available_memory,
            )
        });

        let mut start_row = 0;
        while start_row < height {
            let end_row = (start_row + chunk_rows).min(height);
            let base = start_row * width;
            let span = (end_row - start_row) * width;

            let cov_chunks: Vec<Option<&[f32]>> = self
                .frames
                .iter()
                .map(|f| f.coverage.as_ref().map(|p| p.chunk(base, base + span)))
                .collect();

            let cov_out = &mut coverage.pixels_mut()[base..base + span];
            cov_out
                .par_chunks_mut(width)
                .enumerate()
                .for_each(|(row_in_chunk, cov_row)| {
                    let row_base = row_in_chunk * width;
                    for (px, output) in cov_row.iter_mut().enumerate() {
                        let local = row_base + px;
                        let count = cov_chunks
                            .iter()
                            .filter(|cov| {
                                cov.map_or(1.0, |map| map[local]) > MIN_CONTRIBUTING_COVERAGE
                            })
                            .count();
                        *output = count as f32 * inv_frames;
                    }
                });

            start_row = end_row;
        }

        StackProduct {
            image,
            coverage,
            weight,
            linear_variance,
        }
    }

    /// Warp-quality-aware combine: coverage gates inclusion, while confidence scales statistical
    /// weight independently. Effective weight and linear variance use each channel reducer's
    /// actual survivor set. A pixel no frame supports gets `0`.
    pub(crate) fn process_chunked_weighted<Combine>(
        &self,
        weights: Option<&[f32]>,
        frame_norms: Option<&[FrameNorm]>,
        combine: Combine,
    ) -> LightCombineOutput
    where
        Combine: Fn(&mut [f32], &[f32], &mut ScratchBuffers) -> CombinedSample + Sync,
    {
        if let Some(w) = weights {
            assert_eq!(
                w.len(),
                self.frames.len(),
                "Weight count must match frame count"
            );
        }
        // An in-memory stack is one chunk, so the per-chunk cancel check in
        // `process_chunks` can't interrupt the combine — poll per row here too.
        let cancel = self.core.cancel.clone();
        let dimensions = self.core.dimensions;
        let memory = weighted_chunk_memory_layout(&self.frames, dimensions.channels());
        // Coverage sizing must reuse this pre-output snapshot or resident planes are charged twice.
        let chunk_available_memory = self.core.chunk_available_memory();
        let mut output_weight = PixelData::new_default(
            dimensions.width(),
            dimensions.height(),
            dimensions.channels(),
        );
        let mut output_linear_variance = PixelData::new_default(
            dimensions.width(),
            dimensions.height(),
            dimensions.channels(),
        );
        let pixels =
            self.core.process_chunks(
                &self.frames,
                |frame| &frame.channels,
                memory,
                chunk_available_memory,
                |output_slice, ctx| {
                    let ChunkContext {
                        frames,
                        width,
                        channel,
                        pixel_offset,
                    } = ctx;
                    let frame_count = frames.len();
                    let chunk_pixels = output_slice.len();
                    // Per-frame support and confidence slices; `None` means full support/unit confidence.
                    let coverage: Vec<Option<&[f32]>> =
                        self.frames
                            .iter()
                            .map(|frame| {
                                frame.coverage.as_ref().map(|plane| {
                                    plane.chunk(pixel_offset, pixel_offset + chunk_pixels)
                                })
                            })
                            .collect();
                    let confidence: Vec<Option<&[f32]>> =
                        self.frames
                            .iter()
                            .map(|frame| {
                                frame.confidence.as_ref().map(|plane| {
                                    plane.chunk(pixel_offset, pixel_offset + chunk_pixels)
                                })
                            })
                            .collect();
                    let weight_slice = &mut output_weight.channel_mut(channel).pixels_mut()
                        [pixel_offset..pixel_offset + chunk_pixels];
                    let linear_variance_slice = &mut output_linear_variance
                        .channel_mut(channel)
                        .pixels_mut()[pixel_offset..pixel_offset + chunk_pixels];
                    output_slice
                    .par_chunks_mut(width)
                    .zip(weight_slice.par_chunks_mut(width))
                    .zip(linear_variance_slice.par_chunks_mut(width))
                    .enumerate()
                    .for_each_init(
                        || {
                            (
                                vec![0.0f32; frame_count],
                                vec![0.0f32; frame_count],
                                ScratchBuffers::new(frame_count),
                            )
                        },
                        |(values, eff_weights, scratch),
                         (row_in_chunk, ((row_output, row_weight), row_linear_variance))| {
                            // Cancelled: skip the row's work (output stays zero; the
                            // caller discards the partial result and reports Cancelled).
                            if cancel.is_cancelled() {
                                return;
                            }
                            let row_offset = row_in_chunk * width;
                            for pixel_in_row in 0..width {
                                let pixel_idx = row_offset + pixel_in_row;
                                let mut covered = 0usize;
                                for (frame_idx, chunk) in frames.iter().enumerate() {
                                    let c = match coverage[frame_idx] {
                                        Some(map) => map[pixel_idx],
                                        None => 1.0,
                                    };
                                    let q = match confidence[frame_idx] {
                                        Some(map) => map[pixel_idx],
                                        None => 1.0,
                                    };
                                    if c > MIN_CONTRIBUTING_COVERAGE && q > 0.0 {
                                        let v = match frame_norms {
                                            Some(fnm) => {
                                                let cn = fnm[frame_idx].channels[channel];
                                                chunk[pixel_idx] * cn.gain + cn.offset
                                            }
                                            None => chunk[pixel_idx],
                                        };
                                        values[covered] = v;
                                        eff_weights[covered] =
                                            weights.map_or(1.0, |w| w[frame_idx]) * q;
                                        covered += 1;
                                    }
                                }
                                let sample = if covered == 0 {
                                    CombinedSample::default()
                                } else {
                                    debug_assert!(
                                        values[..covered].iter().all(|v| v.is_finite()),
                                        "non-finite pixel value entered the combine",
                                    );
                                    combine(
                                        &mut values[..covered],
                                        &eff_weights[..covered],
                                        scratch,
                                    )
                                };
                                row_output[pixel_in_row] = sample.value;
                                row_weight[pixel_in_row] = sample.weight;
                                row_linear_variance[pixel_in_row] = sample.linear_variance;
                            }
                        },
                    );
                },
            );
        LightCombineOutput {
            pixels,
            weight: output_weight,
            linear_variance: Some(output_linear_variance),
            chunk_available_memory,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
