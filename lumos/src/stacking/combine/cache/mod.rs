//! Chunked combine engine for resident and memory-mapped stacking frames.

mod loader;

use common::CancelToken;
use imaginarium::Buffer2;
use rayon::prelude::*;

use crate::io::astro_image::{AstroImage, AstroImageMetadata, ImageDimensions, PixelData};
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::error::Error;
use crate::stacking::combine::progress::{ProgressCallback, StackingStage, report_progress};
use crate::stacking::combine::stack::{FrameNorm, StackFrame};
use crate::stacking::frame_store::{
    FrameStats, SpillDirectory, StoredFrame, StoredLightFrame, StoredPlane, optimal_chunk_rows,
};
use crate::stacking::product::StackProduct;

/// Per-thread scratch buffers for stacking combine closures.
///
/// Allocated once per rayon thread via `for_each_init` and reused across all pixels.
#[derive(Debug)]
pub(crate) struct ScratchBuffers {
    /// Tracks original frame indices after rejection reordering.
    pub indices: Vec<usize>,
    /// General-purpose f32 scratch (e.g. winsorized working copy).
    pub floats_a: Vec<f32>,
    /// Second f32 scratch (e.g. median/MAD computation).
    pub floats_b: Vec<f32>,
    /// usize scratch (large-N `sort_with_indices` permutation).
    pub usize_a: Vec<usize>,
    /// Second usize scratch (large-N `sort_with_indices` index copy).
    pub usize_b: Vec<usize>,
}

impl ScratchBuffers {
    fn new(frame_count: usize) -> Self {
        Self {
            indices: Vec::with_capacity(frame_count),
            floats_a: Vec::with_capacity(frame_count),
            floats_b: Vec::with_capacity(frame_count),
            usize_a: Vec::with_capacity(frame_count),
            usize_b: Vec::with_capacity(frame_count),
        }
    }
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
    pub(crate) metadata: AstroImageMetadata,
    /// Per-frame channel statistics (median + MAD), computed at load time.
    pub(crate) channel_stats: Vec<FrameStats>,
    /// Configuration for cache operations.
    pub(crate) config: CacheConfig,
    /// Progress callback.
    pub(crate) progress: ProgressCallback,
    /// Cooperative cancel flag, polled by [`Self::process_chunks`] between
    /// chunks. `None` (the construction default) = never cancel; a public
    /// stacking entry sets it from the caller's flag before the combine.
    pub(crate) cancel: CancelToken,
}

/// Plain calibration cache: `CfaImage` frames, channels only (never coverage), plain combine.
/// Built from disk via [`CfaCache::from_paths`]; stacks to a `CfaImage`.
#[derive(Debug)]
pub(crate) struct CfaCache {
    // Stored planes drop before the spill directory owner in `core`.
    pub(crate) frames: Vec<StoredFrame>,
    pub(crate) core: CacheCore,
}

/// Light-frame stacking cache: `AstroImage` frames with optional per-pixel coverage, coverage-
/// weighted combine. Built in-memory from [`StackFrame`]s ([`LightCache::from_stack_frames`]) or
/// from disk ([`LightCache::from_paths`], coverage `None`); stacks to an `AstroImage`.
#[derive(Debug)]
pub(crate) struct LightCache {
    pub(crate) frames: Vec<StoredLightFrame>,
    pub(crate) core: CacheCore,
}

/// Minimum coverage for a warped frame to contribute at a pixel. Below this the frame's value
/// there is (near-)entirely warp border-fill, so it's *excluded* from the combine rather than
/// down-weighted — otherwise its fill would skew rejection and re-introduce the dark warped-edge
/// ring this weighting exists to remove.
const COVERAGE_EPSILON: f32 = 1e-3;

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

impl CacheCore {
    /// Combine engine: walk the output in memory-bounded row chunks (whole planes for in-memory
    /// stacks, bounded row chunks for disk-backed), gather each frame's channel slice for the
    /// chunk via [`StoredPlane::chunk`], and hand `(output_slice, ChunkContext)` to `process`. The frames
    /// live in the owning cache, so they're passed in. Returns the combined `PixelData`.
    fn process_chunks<F, Channels, Process>(
        &self,
        frames: &[F],
        frame_channels: Channels,
        mut process: Process,
    ) -> PixelData
    where
        Channels: for<'a> Fn(&'a F) -> &'a [StoredPlane] + Copy,
        Process: FnMut(&mut [f32], ChunkContext),
    {
        let dims = self.dimensions;
        let frame_count = frames.len();
        let width = dims.size.x;
        let height = dims.size.y;

        // Whole-plane chunks in RAM; for disk-backed stacks size chunks to the memory budget
        // (queried only here, where it's used — an in-memory stack skips the sysinfo read).
        let chunk_rows = if self.spill_directory.is_none() {
            height
        } else {
            let available_memory = self.config.get_available_memory();
            optimal_chunk_rows(width, 1, frame_count, available_memory)
        };

        let mut output = PixelData::new_default(width, height, dims.channels);
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
        let width = self.dimensions.size.x;
        channels(&frames[frame_idx])[channel].chunk(start_row * width, end_row * width)
    }
}

impl CfaCache {
    /// Plain per-pixel combine: every frame contributes at every pixel (CFA frames have no
    /// coverage). Optional `weights` provide per-frame weights; `frame_norms` apply per-frame
    /// affine normalization before combining. Per-channel, parallelized per-row with rayon.
    pub fn process_chunked<Combine>(
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
        self.core.process_chunks(
            &self.frames,
            |frame| &frame.channels,
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
                                // The combine reducers (median/MAD, compensated sums) assume finite
                                // inputs — NaN/Inf would silently corrupt the output (NaN-unsafe
                                // ordering, multiply-through). No production path emits them today
                                // (FITS load sanitizes, flat division is floored, warp border-fills 0),
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
        spill_directory: Option<SpillDirectory>,
        dimensions: ImageDimensions,
        metadata: AstroImageMetadata,
        config: &CacheConfig,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Self {
        let channel_stats = frames.iter().map(|frame| frame.stats.clone()).collect();
        Self {
            frames,
            core: CacheCore {
                spill_directory,
                dimensions,
                metadata,
                channel_stats,
                config: config.clone(),
                progress,
                cancel,
            },
        }
    }

    /// Build an in-memory coverage-weighted cache from [`StackFrame`]s (image + optional
    /// per-pixel coverage), moving channels and coverage into resident storage without copying.
    pub fn from_stack_frames(
        frames: Vec<StackFrame>,
        config: &CacheConfig,
        progress: ProgressCallback,
    ) -> Result<Self, Error> {
        if frames.is_empty() {
            return Err(Error::NoFrames);
        }
        let dimensions = frames[0].image.dimensions();
        let metadata = frames[0].image.metadata.clone();

        for (index, frame) in frames.iter().enumerate() {
            if index > 0 && frame.image.dimensions() != dimensions {
                return Err(Error::DimensionMismatch {
                    index,
                    expected: dimensions,
                    actual: frame.image.dimensions(),
                });
            }
            if let Some(coverage) = &frame.coverage
                && (coverage.width(), coverage.height()) != (dimensions.size.x, dimensions.size.y)
            {
                return Err(Error::CoverageDimensionMismatch {
                    index,
                    expected_width: dimensions.size.x,
                    expected_height: dimensions.size.y,
                    actual_width: coverage.width(),
                    actual_height: coverage.height(),
                });
            }
        }
        let stored = frames
            .into_iter()
            .map(|frame| StoredLightFrame::from_memory(frame.image, frame.coverage))
            .collect::<Vec<_>>();
        let channel_stats = stored.iter().map(|frame| frame.stats.clone()).collect();

        Ok(Self {
            frames: stored,
            core: CacheCore {
                spill_directory: None,
                dimensions,
                metadata,
                channel_stats,
                config: config.clone(),
                progress,
                // Set by the public stacking entry before the combine, if any.
                cancel: CancelToken::never(),
            },
        })
    }

    /// Geometric per-pixel quality planes — `coverage` (fraction of frames contributing, `[0,1]`),
    /// `weight` (`Σ wᵢcᵢ`, the WHT), and `variance` (`Σ(wᵢcᵢ)²/(Σ wᵢcᵢ)²`, output variance per unit
    /// input variance) — matching the `Σwᵢ`/`Σwᵢ²` maps drizzle reports. The weight `wᵢ·cᵢ`
    /// (per-frame weight × per-pixel coverage) doesn't depend on channel, so this rides cheaply on
    /// data the combine already touches and stays one plane each.
    ///
    /// **Pre-rejection:** a frame counts wherever its coverage clears [`COVERAGE_EPSILON`] — the set
    /// the combine draws from *before* outlier rejection narrows it per channel. Computed in a
    /// standalone pass so the combine engine is untouched; under aggressive rejection the variance
    /// is a slight under-estimate (see the per-channel follow-up in `docs/pipeline/roadmap.md`).
    ///
    /// `weights` are the same per-frame weights the combine used (`None` = equal, e.g. median).
    pub(crate) fn finish_product(
        &self,
        image: AstroImage,
        weights: Option<&[f32]>,
    ) -> StackProduct {
        let frame_count = self.frames.len();
        if let Some(w) = weights {
            assert_eq!(w.len(), frame_count, "weight count must match frame count");
        }
        let width = self.core.dimensions.size.x;
        let height = self.core.dimensions.size.y;
        let frame_weight = |f: usize| weights.map_or(1.0, |w| w[f]);

        // No frame carries a coverage map → every pixel is fully covered and the planes are
        // spatially uniform; fill constants and skip the per-pixel pass. (`stack()`-from-disk
        // always lands here, as does an all-reference in-memory stack.)
        if self.frames.iter().all(|f| f.coverage.is_none()) {
            let wsum: f32 = (0..frame_count).map(frame_weight).sum();
            let wsq: f32 = (0..frame_count).map(|f| frame_weight(f).powi(2)).sum();
            let variance = if wsum > 0.0 { wsq / (wsum * wsum) } else { 0.0 };
            return StackProduct {
                image,
                coverage: Buffer2::new_filled(width, height, 1.0),
                weight: Buffer2::new_filled(width, height, wsum),
                variance: Buffer2::new_filled(width, height, variance),
            };
        }

        let mut coverage = Buffer2::new_default(width, height);
        let mut weight = Buffer2::new_default(width, height);
        let mut variance = Buffer2::new_default(width, height);
        let inv_frames = 1.0 / frame_count as f32;

        // Coverage planes share their frame's tier, so they may be mmap-backed: read them in the
        // same row-aligned chunks the combine uses. The output planes are small single-channel RAM
        // buffers, written in parallel across the three at once, one row per task.
        let chunk_rows = if self.core.spill_directory.is_none() {
            height
        } else {
            optimal_chunk_rows(
                width,
                1,
                frame_count,
                self.core.config.get_available_memory(),
            )
        };

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
            let wt_out = &mut weight.pixels_mut()[base..base + span];
            let var_out = &mut variance.pixels_mut()[base..base + span];
            cov_out
                .par_chunks_mut(width)
                .zip(wt_out.par_chunks_mut(width))
                .zip(var_out.par_chunks_mut(width))
                .enumerate()
                .for_each(|(row_in_chunk, ((cov_row, wt_row), var_row))| {
                    let row_base = row_in_chunk * width;
                    for px in 0..width {
                        let local = row_base + px;
                        let mut count = 0u32;
                        let mut wsum = 0.0f32;
                        let mut wsq = 0.0f32;
                        for (frame_idx, cov) in cov_chunks.iter().enumerate() {
                            let c = cov.map_or(1.0, |map| map[local]);
                            if c > COVERAGE_EPSILON {
                                let w = frame_weight(frame_idx) * c;
                                wsum += w;
                                wsq += w * w;
                                count += 1;
                            }
                        }
                        cov_row[px] = count as f32 * inv_frames;
                        wt_row[px] = wsum;
                        var_row[px] = if wsum > 0.0 { wsq / (wsum * wsum) } else { 0.0 };
                    }
                });

            start_row = end_row;
        }

        StackProduct {
            image,
            coverage,
            weight,
            variance,
        }
    }

    /// Coverage-weighted combine: a frame contributes at a pixel only where its coverage exceeds
    /// [`COVERAGE_EPSILON`], weighted by `coverage × per-frame weight`. Excluding sub-ε coverage
    /// keeps warp border-fill out of the rejection set, so warped-frame borders don't drag the
    /// stacked edges dark; a pixel no frame covers gets `0`.
    pub fn process_chunked_weighted<Combine>(
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
        self.core.process_chunks(
            &self.frames,
            |frame| &frame.channels,
            |output_slice, ctx| {
                let ChunkContext {
                    frames,
                    width,
                    channel,
                    pixel_offset,
                } = ctx;
                let frame_count = frames.len();
                let chunk_pixels = output_slice.len();
                // Per-frame coverage slice for this chunk's rows; `None` = fully covered.
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
                output_slice
                    .par_chunks_mut(width)
                    .enumerate()
                    .for_each_init(
                        || {
                            (
                                vec![0.0f32; frame_count],
                                vec![0.0f32; frame_count],
                                ScratchBuffers::new(frame_count),
                            )
                        },
                        |(values, eff_weights, scratch), (row_in_chunk, row_output)| {
                            // Cancelled: skip the row's work (output stays zero; the
                            // caller discards the partial result and reports Cancelled).
                            if cancel.is_cancelled() {
                                return;
                            }
                            let row_offset = row_in_chunk * width;
                            for (pixel_in_row, out) in row_output.iter_mut().enumerate() {
                                let pixel_idx = row_offset + pixel_in_row;
                                let mut covered = 0usize;
                                for (frame_idx, chunk) in frames.iter().enumerate() {
                                    let c = match coverage[frame_idx] {
                                        Some(map) => map[pixel_idx],
                                        None => 1.0,
                                    };
                                    if c > COVERAGE_EPSILON {
                                        let v = match frame_norms {
                                            Some(fnm) => {
                                                let cn = fnm[frame_idx].channels[channel];
                                                chunk[pixel_idx] * cn.gain + cn.offset
                                            }
                                            None => chunk[pixel_idx],
                                        };
                                        values[covered] = v;
                                        eff_weights[covered] =
                                            weights.map_or(1.0, |w| w[frame_idx]) * c;
                                        covered += 1;
                                    }
                                }
                                *out = if covered == 0 {
                                    0.0
                                } else {
                                    debug_assert!(
                                        values[..covered].iter().all(|v| v.is_finite()),
                                        "non-finite pixel value entered the combine",
                                    );
                                    combine(
                                        &mut values[..covered],
                                        Some(&eff_weights[..covered]),
                                        scratch,
                                    )
                                };
                            }
                        },
                    );
            },
        )
    }
}

/// Get the source file's modification time as seconds since epoch.

#[cfg(test)]
pub(crate) mod tests;
