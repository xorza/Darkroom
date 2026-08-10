//! Walking the output in memory-bounded row chunks.
//!
//! The part of the combine that does not depend on what is being combined: pick a chunk height
//! the budget allows, gather each frame's slice of the chunk through [`StoredPlane::chunk`] — the
//! one call that hides whether a plane is resident or memory-mapped — and hand the pair to the
//! reducer. An in-memory stack is a single chunk; a spilled one is as many as the budget dictates.

use common::CancelToken;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear_pixels::LinearPixels;
use crate::memory::ChunkMemoryLayout;
use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::frame_store::StoredFrame;
use crate::stacking::frame_store::spill::SpillDirectory;
use crate::stacking::frame_store::stored_plane::StoredPlane;
use crate::stacking::progress::{ProgressCallback, StackingStage};
use crate::stacking::stack_product::quality_planes::QualityPlanes;

/// Shared cache context + combine engine — everything that doesn't depend on the frame type.
/// Owned by composition inside [`FrameCache`](super::FrameCache); all frames share one tier, and
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

/// Per-chunk context handed to the [`CacheCore::process_chunks`] closure: the input frame
/// slices for this chunk plus the geometry to map a within-chunk pixel to a global frame index.
#[derive(Debug)]
pub(super) struct ChunkContext<'a> {
    /// One channel slice per frame for this chunk; `frames.len()` is the frame count.
    pub(super) frames: &'a [&'a [f32]],
    /// Row width in pixels.
    pub(super) width: usize,
    /// Channel currently being combined.
    pub(super) channel: usize,
    /// Global pixel index of this chunk's first pixel — for indexing full-frame,
    /// channel-independent maps such as coverage.
    pub(super) pixel_offset: usize,
}

/// Planes a combine keeps resident per output channel: the combined pixels, plus whichever
/// quality planes were asked for and so were allocated up front.
///
/// Coverage is not among them — it has no per-channel plane and is accumulated after the combine,
/// in [`FrameCache::finish_product`](super::FrameCache::finish_product).
pub(crate) fn resident_planes_per_channel(planes: QualityPlanes) -> usize {
    1 + usize::from(planes.weight) + usize::from(planes.variance)
}

/// Each frame's slice of one warp-quality plane over `[start, end)`, `None` where the frame
/// carries no such plane.
///
/// `plane` picks which of the two a frame's slot is — the combine and the coverage pass both
/// gather them the same way and differ only in that choice.
pub(crate) fn quality_plane_chunks(
    frames: &[StoredFrame],
    plane: fn(&StoredFrame) -> Option<&StoredPlane>,
    start: usize,
    end: usize,
) -> Vec<Option<&[f32]>> {
    frames
        .iter()
        .map(|frame| plane(frame).map(|plane| plane.chunk(start, end)))
        .collect()
}

/// What the combine pass holds: one input plane per frame channel, plus one more for each of that
/// frame's coverage and confidence planes, against the resident output planes.
pub(crate) fn weighted_chunk_memory_layout(
    frames: &[StoredFrame],
    output_channels: usize,
    planes: QualityPlanes,
) -> ChunkMemoryLayout {
    ChunkMemoryLayout {
        input_planes: frames.iter().map(|frame| 1 + frame.quality.count()).sum(),
        resident_planes: output_channels * resident_planes_per_channel(planes),
    }
}

/// What the coverage pass holds: one input plane per frame that carries coverage, against the
/// combine's residents — which are all still alive at that point — plus the single coverage plane
/// being accumulated.
pub(crate) fn coverage_chunk_memory_layout(
    frames: &[StoredFrame],
    output_channels: usize,
    planes: QualityPlanes,
) -> ChunkMemoryLayout {
    ChunkMemoryLayout {
        input_planes: frames
            .iter()
            .filter(|frame| frame.quality.coverage.is_some())
            .count(),
        resident_planes: output_channels * resident_planes_per_channel(planes) + 1,
    }
}

impl CacheCore {
    pub(super) fn chunk_available_memory(&self) -> Option<u64> {
        self.spill_directory
            .as_ref()
            .map(|_| self.config.get_available_memory())
    }

    /// Combine engine: walk the output in memory-bounded row chunks (whole planes for in-memory
    /// stacks, bounded row chunks for disk-backed), gather each frame's channel slice for the
    /// chunk via [`StoredPlane::chunk`], and hand `(output_slice, ChunkContext)` to `process`. The frames
    /// live in the owning cache, so they're passed in. Returns the combined `LinearPixels`.
    pub(super) fn process_chunks<F, Channels, Process>(
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
            memory.optimal_chunk_rows(dims.size(), available_memory)
        });

        let mut output = LinearPixels::new_zeroed(dims);
        let channel_count = output.channel_count();

        let num_chunks = height.div_ceil(chunk_rows);
        let total_work = num_chunks * channel_count;

        let mut chunks: Vec<&[f32]> = Vec::with_capacity(frame_count);

        self.progress
            .report(0, total_work, StackingStage::Combining);

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
                    StackingStage::Combining,
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
    pub(super) fn read_channel_chunk<'a, F, Channels>(
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
