//! Chunked combine engine for resident and memory-mapped stacking frames.

mod loader;

use common::CancelToken;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::image::linear::LinearImage;
use crate::io::image::linear_pixels::LinearPixels;
use crate::io::image::{ImageDimensions, ImageMetadata};
use crate::stacking::combine::MIN_CONTRIBUTING_COVERAGE;
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::config::Normalization;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::normalization::{FrameNorm, compute_frame_norms};
use crate::stacking::combine::stack::StackFrame;
use crate::stacking::frame_store::{
    ChunkMemoryLayout, SpillDirectory, StackableImage, StoredFrame, StoredPlane, optimal_chunk_rows,
};
use crate::stacking::product::{QualityMap, QualityPlanes, StackProduct};
use crate::stacking::progress::{ProgressCallback, StackingStage};

/// Per-thread scratch buffers for stacking combine closures.
///
/// Allocated once per rayon thread via `for_each_init` and reused across all pixels.
#[derive(Debug, Default)]
pub(crate) struct ScratchBuffers {
    /// Tracks original frame indices after rejection reordering.
    pub(crate) indices: Vec<usize>,
    /// General-purpose f32 scratch (e.g. winsorized working copy).
    pub(crate) floats_a: Vec<f32>,
    /// Second f32 scratch, taken by large-N `sort_with_indices` for its value copy.
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

/// One reduced channel sample: the combined value, how many samples reached it, and — when the
/// caller asked for the quality planes — the effective weight of the survivors.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CombinedSample {
    pub(crate) value: f32,
    /// Samples that survived rejection. Always tracked: it is a count the reducer already knows,
    /// and quantization-noise propagation keys on it.
    pub(crate) survivor_count: usize,
    weight: f32,
    linear_variance: f32,
}

impl CombinedSample {
    /// A reduction whose survivors are all the inputs.
    pub(crate) fn from_all(value: f32, weights: &[f32]) -> Self {
        Self::from_survivors(value, weights, weights.len(), 0..weights.len())
    }

    /// A reduction over `survivor_indices` into `weights`, measuring their effective weight.
    pub(crate) fn from_survivors(
        value: f32,
        weights: &[f32],
        survivor_count: usize,
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
            survivor_count,
            weight,
            linear_variance,
        }
    }

    /// A reduction for a combine that asked for no quality planes: the walk over survivor weights
    /// would produce two numbers nothing reads, and it costs one pass over the frames per pixel.
    pub(crate) fn value_only(value: f32, survivor_count: usize) -> Self {
        Self {
            value,
            survivor_count,
            weight: 0.0,
            linear_variance: 0.0,
        }
    }
}

/// Channel-shaped result of one combine pass, plus the memory snapshot taken before the output
/// planes were allocated. A plane is `None` when [`QualityPlanes`] did not ask for it.
#[derive(Debug)]
pub(crate) struct CombineOutput {
    pub(super) pixels: LinearPixels,
    weight: Option<LinearPixels>,
    linear_variance: Option<LinearPixels>,
    chunk_available_memory: Option<u64>,
}

/// The output rows one combine row-task writes: the combined value, plus whichever ancillary
/// planes were requested. Bundling them keeps one gather loop instead of one per plane subset.
#[derive(Debug)]
struct QualityRows<'a> {
    value: &'a mut [f32],
    weight: Option<&'a mut [f32]>,
    linear_variance: Option<&'a mut [f32]>,
}

/// Shared cache context + combine engine — everything that doesn't depend on the frame type.
/// Owned by composition inside [`FrameCache`]; all frames share one tier, and
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

/// The frames feeding one combine, with their normalization parameters. Calibration masters and
/// registered light stacks share it; a calibration frame simply carries no warp quality planes.
#[derive(Debug)]
pub(crate) struct FrameCache {
    // Stored planes drop before the spill directory owner in `core`.
    pub(crate) frames: Vec<StoredFrame>,
    pub(crate) frame_norms: Option<Vec<FrameNorm>>,
    /// The normalization `frame_norms` was measured for. Kept so the combine can confirm the
    /// `StackConfig` it is handed asks for the normalization the cache was actually built with —
    /// the parameters are fixed at construction and never recomputed.
    pub(crate) normalization: Normalization,
    pub(crate) core: CacheCore,
}

#[derive(Debug)]
pub(crate) struct FrameCacheParams {
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

/// Check a stored frame's shape against the geometry the cache was built for.
///
/// The counterpart to the dimension checks [`FrameCache::from_stack_frames`] makes on
/// caller-supplied images. A stored plane carries no width or height, so this compares plane
/// counts and sample counts instead — enough to guarantee every `chunk(..)` below is in range.
fn validate_stored_geometry(
    frame: &StoredFrame,
    dimensions: ImageDimensions,
    index: usize,
) -> Result<(), Error> {
    if frame.channels.len() != dimensions.channels() {
        return Err(Error::StoredFrameChannels {
            index,
            expected: dimensions.channels(),
            actual: frame.channels.len(),
        });
    }
    let expected = dimensions.pixel_count();
    let planes = frame
        .channels
        .iter()
        .map(|plane| ("a channel", plane))
        .chain(frame.coverage.as_ref().map(|plane| ("coverage", plane)))
        .chain(frame.confidence.as_ref().map(|plane| ("confidence", plane)));
    for (plane_name, plane) in planes {
        if plane.samples() != expected {
            return Err(Error::StoredFramePlaneSamples {
                index,
                plane: plane_name,
                expected,
                actual: plane.samples(),
            });
        }
    }
    Ok(())
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
    frames: &[StoredFrame],
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
    /// live in the owning cache, so they're passed in. Returns the combined `LinearPixels`.
    fn process_chunks<F, Channels, Process>(
        &self,
        frames: &[F],
        frame_channels: Channels,
        memory: ChunkMemoryLayout,
        available_memory: Option<u64>,
        mut process: Process,
    ) -> LinearPixels
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

        let mut output = LinearPixels::new_zeroed(dims);
        let channel_count = output.channel_count();

        let num_chunks = height.div_ceil(chunk_rows);
        let total_work = num_chunks * channel_count;

        let mut chunks: Vec<&[f32]> = Vec::with_capacity(frame_count);

        self.progress
            .report(0, total_work, StackingStage::Processing);

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

                self.progress.report(
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

impl FrameCache {
    /// Build a cache from frames already placed in the shared frame store.
    pub(crate) fn from_stored_frames(
        frames: Vec<StoredFrame>,
        params: FrameCacheParams,
    ) -> Result<Self, Error> {
        let FrameCacheParams {
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
            // Geometry before contents: every read below and in the combine slices a plane to
            // `pixel_count`, so a short plane would panic out of a slice index rather than
            // reporting which frame was the wrong shape.
            validate_stored_geometry(frame, dimensions, index)?;
            validate_stored_samples(&frame.channels, dimensions.pixel_count(), index, &cancel)?;
            // Same guarantee `from_stack_frames` gives caller-supplied planes: coverage in
            // `[0, 1]` and confidence non-negative, so the gate and the weight multiplier below
            // can't be handed a value that silently corrupts the combine.
            for (plane_name, plane) in [
                ("coverage", frame.coverage.as_ref()),
                ("confidence", frame.confidence.as_ref()),
            ] {
                if let Some(plane) = plane {
                    validate_warp_plane_values(
                        index,
                        plane_name,
                        plane.chunk(0, dimensions.pixel_count()),
                        &cancel,
                    )?;
                }
            }
        }
        let frame_norms = compute_frame_norms(&frames, dimensions, normalization, &cancel)?;
        Ok(Self {
            frames,
            frame_norms,
            normalization,
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
                StoredFrame::from_memory(
                    frame.image,
                    frame.coverage,
                    frame.confidence,
                    frame.source_stats,
                )
            })
            .collect::<Vec<_>>();
        let frame_norms = compute_frame_norms(&stored, dimensions, normalization, &cancel)?;

        Ok(Self {
            frames: stored,
            frame_norms,
            normalization,
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
    pub(crate) fn finish_product(
        &self,
        combined: CombineOutput,
        planes: QualityPlanes,
        quantization_sigma: Option<f32>,
    ) -> StackProduct {
        let CombineOutput {
            pixels,
            weight: weight_pixels,
            linear_variance: linear_variance_pixels,
            chunk_available_memory,
        } = combined;
        let dimensions = self.core.dimensions;
        let image = LinearImage {
            metadata: self.core.metadata.clone(),
            pixels,
        };
        let weight = weight_pixels.map(QualityMap::from_pixels);
        let linear_variance = linear_variance_pixels.map(QualityMap::from_pixels);
        let frame_count = self.frames.len();
        let width = dimensions.width();
        let height = dimensions.height();

        // No frame carries support, so every pixel is fully covered. The plane is still
        // materialized when asked for, since `coverage` has no uniform representation.
        if !planes.coverage || self.frames.iter().all(|frame| frame.coverage.is_none()) {
            return StackProduct {
                image,
                coverage: planes
                    .coverage
                    .then(|| Buffer2::new_filled(width, height, 1.0)),
                weight,
                linear_variance,
                quantization_sigma,
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
            coverage: Some(coverage),
            weight,
            linear_variance,
            quantization_sigma,
        }
    }

    /// The combine: for each output pixel, gather the frames that cover it, hand them to
    /// `combine`, and write the reduced value plus whichever [`QualityPlanes`] were requested.
    ///
    /// Coverage gates a frame's inclusion while confidence scales its statistical weight
    /// independently; a frame carrying neither plane contributes everywhere at unit confidence,
    /// which is what lets calibration masters and registered light stacks share this loop. A
    /// pixel no frame supports gets `0`.
    pub(crate) fn process_chunked<Combine>(
        &self,
        weights: Option<&[f32]>,
        frame_norms: Option<&[FrameNorm]>,
        planes: QualityPlanes,
        combine: Combine,
    ) -> CombineOutput
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
        let mut output_weight = planes.weight.then(|| LinearPixels::new_zeroed(dimensions));
        let mut output_linear_variance = planes
            .variance
            .then(|| LinearPixels::new_zeroed(dimensions));
        let pixels = self.core.process_chunks(
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
                let coverage: Vec<Option<&[f32]>> = self
                    .frames
                    .iter()
                    .map(|frame| {
                        frame
                            .coverage
                            .as_ref()
                            .map(|plane| plane.chunk(pixel_offset, pixel_offset + chunk_pixels))
                    })
                    .collect();
                let confidence: Vec<Option<&[f32]>> = self
                    .frames
                    .iter()
                    .map(|frame| {
                        frame
                            .confidence
                            .as_ref()
                            .map(|plane| plane.chunk(pixel_offset, pixel_offset + chunk_pixels))
                    })
                    .collect();
                // One row bundle per output row, so an unrequested plane simply has no slice
                // to write instead of needing its own copy of the gather loop below.
                let mut rows: Vec<QualityRows<'_>> = output_slice
                    .chunks_mut(width)
                    .map(|value| QualityRows {
                        value,
                        weight: None,
                        linear_variance: None,
                    })
                    .collect();
                if let Some(plane) = output_weight.as_mut() {
                    let slice = &mut plane.channel_mut(channel).pixels_mut()
                        [pixel_offset..pixel_offset + chunk_pixels];
                    for (row, chunk) in rows.iter_mut().zip(slice.chunks_mut(width)) {
                        row.weight = Some(chunk);
                    }
                }
                if let Some(plane) = output_linear_variance.as_mut() {
                    let slice = &mut plane.channel_mut(channel).pixels_mut()
                        [pixel_offset..pixel_offset + chunk_pixels];
                    for (row, chunk) in rows.iter_mut().zip(slice.chunks_mut(width)) {
                        row.linear_variance = Some(chunk);
                    }
                }
                rows.into_par_iter().enumerate().for_each_init(
                    || {
                        (
                            vec![0.0f32; frame_count],
                            vec![0.0f32; frame_count],
                            ScratchBuffers::new(frame_count),
                        )
                    },
                    |(values, eff_weights, scratch), (row_in_chunk, mut row)| {
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
                                combine(&mut values[..covered], &eff_weights[..covered], scratch)
                            };
                            row.value[pixel_in_row] = sample.value;
                            if let Some(weight) = row.weight.as_deref_mut() {
                                weight[pixel_in_row] = sample.weight;
                            }
                            if let Some(variance) = row.linear_variance.as_deref_mut() {
                                variance[pixel_in_row] = sample.linear_variance;
                            }
                        }
                    },
                );
            },
        );
        CombineOutput {
            pixels,
            weight: output_weight,
            linear_variance: output_linear_variance,
            chunk_available_memory,
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;

    use crate::stacking::combine::cache::{CacheCore, FrameCache};
    use crate::stacking::combine::cache_config::CacheConfig;
    use crate::stacking::combine::config::Normalization;
    use crate::stacking::combine::normalization::compute_frame_norms;
    use crate::stacking::frame_store::{FrameStats, StackableImage, StoredFrame};
    use crate::stacking::progress::ProgressCallback;

    /// An in-memory [`FrameCache`] over already-decoded frames — the shape `from_paths` builds,
    /// without the file round-trip. The frames carry no warp quality, so this is the plain-combine
    /// cache the calibration path uses.
    pub(crate) fn cache_from_images<I: StackableImage>(
        images: Vec<I>,
        normalization: Normalization,
    ) -> FrameCache {
        let dimensions = images[0].dimensions();
        let metadata = images[0].metadata().clone();
        let frames: Vec<StoredFrame> = images
            .into_iter()
            .map(|image| {
                let source_stats = FrameStats::measure(&image);
                StoredFrame::from_memory(image, None, None, source_stats)
            })
            .collect();
        let core = CacheCore {
            spill_directory: None,
            dimensions,
            metadata,
            config: CacheConfig::default(),
            progress: ProgressCallback::default(),
            cancel: CancelToken::never(),
        };
        let frame_norms = compute_frame_norms(&frames, dimensions, normalization, &core.cancel)
            .expect("frames without coverage have no failing normalization path");
        FrameCache {
            frames,
            frame_norms,
            normalization,
            core,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
