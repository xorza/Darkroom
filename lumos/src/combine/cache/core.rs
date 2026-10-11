//! Walking the output in memory-bounded row chunks.
//!
//! The part of the combine that does not depend on what is being combined: pick a chunk height
//! the budget allows, take each frame's part of the chunk as a [`FrameChunk`], and hand it to the
//! reducer. An in-memory stack is a single chunk, borrowed from its planes; a spilled one is as
//! many as the budget dictates, each copied out of the planes' maps by reader threads while the
//! chunk before it is reduced.

use std::mem;
use std::ops::Range;
use std::thread;

use common::CancelToken;

use crate::combine::cache::frame_chunk::{ChunkCopy, FrameChunk};
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::tier::Tier;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear_pixels::LinearPixels;
use crate::memory::chunk_memory_layout::{ChunkMemoryLayout, ChunkRows};
use crate::progress::progress_callback::ProgressCallback;
use crate::progress::stacking_progress::StackingStage;

/// Shared cache context + combine engine — everything that doesn't depend on the frame type.
/// Owned by composition inside [`FrameCache`](super::FrameCache); all frames share one tier.
#[derive(Debug)]
pub(crate) struct CacheCore {
    pub(crate) tier: Tier,
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

/// Per-chunk context handed to the [`CacheCore::process_chunks`] closure: each frame's samples of
/// the chunk in the channel being combined, its whole [`FrameChunk`], and the geometry to map a
/// within-chunk pixel to a global frame index.
#[derive(Debug)]
pub(super) struct ChunkContext<'a> {
    /// One channel slice per frame for this chunk; `frames.len()` is the frame count.
    pub(super) frames: &'a [&'a [f32]],
    /// Each frame's chunk, its quality planes and flags with it.
    pub(super) chunks: &'a [FrameChunk<'a>],
    /// Row width in pixels.
    pub(super) width: usize,
    /// Channel currently being combined.
    pub(super) channel: usize,
    /// Global pixel index of this chunk's first pixel — for indexing full-frame,
    /// channel-independent maps such as coverage.
    pub(super) pixel_offset: usize,
}

impl CacheCore {
    /// The rows a pass holding `memory` reads at once: the whole image for a resident stack, and
    /// the rows the chunk budget leaves room for on the disk tier, where two chunks are held at
    /// once — the one reduced and the next one read. A pass the floor puts past the budget says
    /// so in the log.
    pub(super) fn chunk_rows(&self, memory: ChunkMemoryLayout) -> ChunkRows {
        let Some(chunk_memory) = self.tier.chunk_memory() else {
            return ChunkRows {
                rows: self.dimensions.height(),
                overcommit_bytes: 0,
            };
        };
        let held = ChunkMemoryLayout {
            input_bytes: 2 * memory.input_bytes,
            ..memory
        };
        let chunking = held.chunk_rows(self.dimensions.size(), chunk_memory);
        if chunking.overcommit_bytes > 0 {
            tracing::warn!(
                rows = chunking.rows,
                overcommit_bytes = chunking.overcommit_bytes,
                "the smallest combine chunk does not fit the memory budget"
            );
        }
        chunking
    }

    /// Combine engine: walk the output in chunks of `chunk_rows` rows (see [`Self::chunk_rows`]),
    /// and every channel within a chunk, and hand `(output_slice, ChunkContext)` to `process`. The
    /// frames live in the owning cache, so they're passed in. Returns the combined `LinearPixels`.
    ///
    /// Chunk-outer, so a frame's quality planes and flags, which every channel reads, are taken
    /// once per chunk rather than once per channel. On the disk tier a chunk is copied out of the
    /// maps ([`ChunkCopy`]) on reader threads while the chunk before it is reduced, so the reads
    /// overlap the reduction and the bytes they bring in sit in the two buffers the budget counts.
    pub(super) fn process_chunks<Process>(
        &self,
        frames: &[StoredFrame],
        chunk_rows: usize,
        mut process: Process,
    ) -> LinearPixels
    where
        Process: FnMut(&mut [f32], ChunkContext<'_>),
    {
        let dims = self.dimensions;
        let width = dims.width();
        let height = dims.height();
        let mut output = LinearPixels::new_zeroed(dims);
        let num_chunks = height.div_ceil(chunk_rows);
        let range = |chunk: usize| {
            let start_row = chunk * chunk_rows;
            let end_row = (start_row + chunk_rows).min(height);
            start_row * width..end_row * width
        };
        if !self.tier.spills() {
            for chunk in 0..num_chunks {
                let views: Vec<FrameChunk<'_>> = frames
                    .iter()
                    .map(|frame| FrameChunk::borrowed(frame, range(chunk)))
                    .collect();
                if !self.reduce_chunk(
                    &mut output,
                    &views,
                    range(chunk),
                    chunk,
                    num_chunks,
                    &mut process,
                ) {
                    break;
                }
            }
            return output;
        }
        let chunk_pixels = range(0).len();
        let mut current = ChunkCopy::new(frames, chunk_pixels);
        let mut next = ChunkCopy::new(frames, chunk_pixels);
        current.fill(frames, range(0));
        for chunk in 0..num_chunks {
            let completed = thread::scope(|scope| {
                let reading = (chunk + 1 < num_chunks)
                    .then(|| scope.spawn(|| next.fill(frames, range(chunk + 1))));
                let views = current.views(frames);
                let completed = self.reduce_chunk(
                    &mut output,
                    &views,
                    range(chunk),
                    chunk,
                    num_chunks,
                    &mut process,
                );
                if let Some(reading) = reading {
                    reading.join().expect("the chunk reader does not panic");
                }
                completed
            });
            if !completed {
                break;
            }
            mem::swap(&mut current, &mut next);
        }
        output
    }

    /// Hand chunk `chunk` of `chunks`, over `pixels`, to `process` channel by channel, reporting
    /// each; `false` once the run is cancelled, the in-flight channel completed and the partial
    /// output left for the caller to discard.
    fn reduce_chunk<Process>(
        &self,
        output: &mut LinearPixels,
        views: &[FrameChunk<'_>],
        pixels: Range<usize>,
        chunk: usize,
        chunks: usize,
        process: &mut Process,
    ) -> bool
    where
        Process: FnMut(&mut [f32], ChunkContext<'_>),
    {
        let channel_count = output.channel_count();
        let mut samples: Vec<&[f32]> = Vec::with_capacity(views.len());
        for channel in 0..channel_count {
            samples.clear();
            samples.extend(views.iter().map(|view| view.channels[channel]));
            process(
                &mut output.channel_mut(channel).pixels_mut()[pixels.clone()],
                ChunkContext {
                    frames: &samples,
                    chunks: views,
                    width: self.dimensions.width(),
                    channel,
                    pixel_offset: pixels.start,
                },
            );
            self.progress.report(
                chunk * channel_count + channel + 1,
                chunks * channel_count,
                StackingStage::Combining,
            );
            if self.cancel.is_cancelled() {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use common::CancelToken;

    use crate::combine::cache::core::CacheCore;
    use crate::frame_store::tier::Tier;
    use crate::io::image::image_dimensions::ImageDimensions;
    use crate::io::image::image_metadata::ImageMetadata;
    use crate::progress::progress_callback::ProgressCallback;

    impl CacheCore {
        /// A core over frames of `dimensions` on `tier`, with default metadata, no progress
        /// reported and no cancel.
        pub(crate) fn plain(tier: Tier, dimensions: ImageDimensions) -> Self {
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
