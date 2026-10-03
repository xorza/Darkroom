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
use crate::memory::run_memory::RunMemory;
use crate::stacking::frame_store::spill_directory::SpillDirectory;
use crate::stacking::frame_store::stored_frame::StoredFrame;
use crate::stacking::progress::{ProgressCallback, StackingStage};

/// Shared cache context + combine engine — everything that doesn't depend on the frame type.
/// Owned by composition inside [`FrameCache`](super::FrameCache); all frames share one tier.
#[derive(Debug)]
pub(crate) struct CacheCore {
    pub(crate) tier: CacheTier,
    /// Image dimensions (same for all frames).
    pub(crate) dimensions: ImageDimensions,
    /// Metadata from the first frame.
    pub(crate) metadata: ImageMetadata,
    /// Progress callback.
    pub(crate) progress: ProgressCallback,
    /// Cooperative cancel flag, present during validation and normalization and polled by
    /// [`Self::process_chunks`] during the combine.
    pub(crate) cancel: CancelToken,
}

/// Where a cache's frames live, and for spilled ones what the combine sizes its row chunks against.
#[derive(Debug)]
pub(crate) enum CacheTier {
    /// Every plane in RAM: the combine walks whole planes and needs no budget.
    Resident,
    /// Planes memory-mapped from files in `directory`, read in row chunks sized against
    /// `chunk_memory`: the run's planning figure, one number for the combine and for the coverage
    /// pass after it.
    Spilled {
        #[cfg_attr(
            not(test),
            expect(
                dead_code,
                reason = "held, not read: the directory outlives the memory maps in the frames"
            )
        )]
        directory: SpillDirectory,
        chunk_memory: u64,
    },
}

impl CacheTier {
    /// The tier of frames spilled to `directory`, or resident for `None`, under `memory`.
    pub(crate) fn of(directory: Option<SpillDirectory>, memory: RunMemory) -> Self {
        directory.map_or(Self::Resident, |directory| Self::Spilled {
            directory,
            chunk_memory: memory.planning(),
        })
    }

    /// What a spilled combine sizes its row chunks against; `None` for a resident one.
    pub(crate) const fn chunk_memory(&self) -> Option<u64> {
        match self {
            Self::Resident => None,
            Self::Spilled { chunk_memory, .. } => Some(*chunk_memory),
        }
    }

    pub(crate) const fn spills(&self) -> bool {
        matches!(self, Self::Spilled { .. })
    }
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

impl CacheCore {
    /// Combine engine: walk the output in memory-bounded row chunks (whole planes for in-memory
    /// stacks, bounded row chunks for disk-backed), gather each frame's channel slice for the chunk
    /// via [`StoredPlane::chunk`], and hand `(output_slice, ChunkContext)` to `process`. The frames
    /// live in the owning cache, so they're passed in. Returns the combined `LinearPixels`.
    pub(super) fn process_chunks<Process>(
        &self,
        frames: &[StoredFrame],
        memory: ChunkMemoryLayout,
        chunk_memory: Option<u64>,
        mut process: Process,
    ) -> LinearPixels
    where
        Process: FnMut(&mut [f32], ChunkContext<'_>),
    {
        let dims = self.dimensions;
        let frame_count = frames.len();
        let width = dims.width();
        let height = dims.height();

        let chunk_rows = chunk_memory.map_or(height, |chunk_memory| {
            memory.optimal_chunk_rows(dims.size(), chunk_memory)
        });

        let mut output = LinearPixels::new_zeroed(dims);
        let channel_count = output.channel_count();

        let num_chunks = height.div_ceil(chunk_rows);
        let total_work = num_chunks * channel_count;

        let mut chunks: Vec<&[f32]> = Vec::with_capacity(frame_count);

        for channel in 0..channel_count {
            for chunk_idx in 0..num_chunks {
                let start_row = chunk_idx * chunk_rows;
                let end_row = (start_row + chunk_rows).min(height);
                let rows_in_chunk = end_row - start_row;
                let pixels_in_chunk = rows_in_chunk * width;

                chunks.clear();
                chunks.extend(frames.iter().map(|frame| {
                    frame.channels[channel].chunk(start_row * width, end_row * width)
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
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;

    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::io::image::image_metadata::ImageMetadata;
    use crate::stacking::combine::cache::core::{CacheCore, CacheTier};
    use crate::stacking::progress::ProgressCallback;

    impl CacheCore {
        /// A core over frames of `dimensions` on `tier`, with default metadata, no progress
        /// reported and no cancel.
        pub(crate) fn plain(tier: CacheTier, dimensions: ImageDimensions) -> Self {
            Self {
                tier,
                dimensions,
                metadata: ImageMetadata::default(),
                progress: ProgressCallback::default(),
                cancel: CancelToken::never(),
            }
        }
    }
}
