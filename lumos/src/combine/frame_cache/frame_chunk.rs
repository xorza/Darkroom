//! [`FrameChunk`]: one frame over one chunk of rows, as the combine reads it.

use std::convert::Infallible;
use std::ops::Range;
use std::thread;

use arrayvec::ArrayVec;

use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::stored_frame::StoredFrame;

/// One frame over one chunk of pixels: every channel, its quality planes and its flags, each the
/// chunk's part alone — borrowed from planes in RAM, or from a [`ChunkCopy`] of planes on disk.
#[derive(Debug)]
pub(crate) struct FrameChunk<'a> {
    pub(crate) channels: ArrayVec<&'a [f32], 3>,
    pub(crate) quality: FrameQuality<&'a [f32]>,
    pub(crate) flags: Option<&'a [u8]>,
}

impl<'a> FrameChunk<'a> {
    /// `frame`'s pixels `range`, read in place from its planes.
    pub(crate) fn borrowed(frame: &'a StoredFrame, range: Range<usize>) -> Self {
        let Range { start, end } = range;
        Self {
            channels: frame
                .channels
                .iter()
                .map(|plane| plane.chunk(start, end))
                .collect(),
            quality: frame
                .quality
                .try_map(|_, plane| Ok::<_, Infallible>(plane.chunk(start, end)))
                .unwrap_or_else(|never| match never {}),
            flags: frame.flags.as_ref().map(|plane| plane.chunk(start, end)),
        }
    }
}

/// The threads a chunk is read on, so a solid-state disk has more than one read in flight. Measured
/// on `bench_spilled_mean_40`, cold, on an NVMe disk with 16 threads: 208 ms with 2, 194 with 4, 198
/// with 8, against 235 for the faults of the maps this replaced.
const READERS: usize = 4;

/// Every frame's planes over one chunk, copied out of their maps into buffers kept from chunk to
/// chunk: the bytes a spilled combine reads, held where its budget counts them rather than in the
/// page cache, and read front to back once per plane, so the cache needs no more than its
/// readahead.
#[derive(Debug)]
pub(crate) struct ChunkCopy {
    frames: Vec<FrameBuffers>,
}

/// One frame's buffers: a channel each, the quality planes in [`FrameQuality::present`] order, and
/// the flags.
#[derive(Debug)]
struct FrameBuffers {
    channels: ArrayVec<Vec<f32>, 3>,
    quality: Vec<Vec<f32>>,
    flags: Option<Vec<u8>>,
}

impl ChunkCopy {
    /// Buffers for `frames`' planes, each sized for a chunk of `pixels` and filled by
    /// [`Self::fill`].
    pub(crate) fn new(frames: &[StoredFrame], pixels: usize) -> Self {
        Self {
            frames: frames
                .iter()
                .map(|frame| FrameBuffers {
                    channels: frame
                        .channels
                        .iter()
                        .map(|_| Vec::with_capacity(pixels))
                        .collect(),
                    quality: frame
                        .quality
                        .present()
                        .map(|_| Vec::with_capacity(pixels))
                        .collect(),
                    flags: frame.flags.as_ref().map(|_| Vec::with_capacity(pixels)),
                })
                .collect(),
        }
    }

    /// Copy `frames`' pixels `range` into the buffers, plane by plane, on [`READERS`] threads of
    /// its own, each taking a run of the frames.
    pub(crate) fn fill(&mut self, frames: &[StoredFrame], range: Range<usize>) {
        debug_assert_eq!(frames.len(), self.frames.len());
        let per_reader = frames.len().div_ceil(READERS).max(1);
        thread::scope(|scope| {
            for (frames, buffers) in frames
                .chunks(per_reader)
                .zip(self.frames.chunks_mut(per_reader))
            {
                let range = range.clone();
                scope.spawn(move || Self::fill_frames(frames, buffers, range));
            }
        });
    }

    fn fill_frames(frames: &[StoredFrame], buffers: &mut [FrameBuffers], range: Range<usize>) {
        let Range { start, end } = range;
        for (frame, buffers) in frames.iter().zip(buffers) {
            for (plane, buffer) in frame.channels.iter().zip(&mut buffers.channels) {
                buffer.clear();
                buffer.extend_from_slice(plane.chunk(start, end));
            }
            for ((_, plane), buffer) in frame.quality.present().zip(&mut buffers.quality) {
                buffer.clear();
                buffer.extend_from_slice(plane.chunk(start, end));
            }
            if let (Some(plane), Some(buffer)) = (&frame.flags, &mut buffers.flags) {
                buffer.clear();
                buffer.extend_from_slice(plane.chunk(start, end));
            }
        }
    }

    /// Each frame's chunk as the last [`Self::fill`] left it, shaped as `frames` are.
    pub(crate) fn views<'a>(&'a self, frames: &[StoredFrame]) -> Vec<FrameChunk<'a>> {
        frames
            .iter()
            .zip(&self.frames)
            .map(|(frame, buffers)| {
                let mut quality = buffers.quality.iter();
                FrameChunk {
                    channels: buffers.channels.iter().map(Vec::as_slice).collect(),
                    quality: frame
                        .quality
                        .try_map(|_, _| {
                            Ok::<_, Infallible>(
                                quality
                                    .next()
                                    .expect("a buffer for every quality plane")
                                    .as_slice(),
                            )
                        })
                        .unwrap_or_else(|never| match never {}),
                    flags: buffers.flags.as_deref(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use arrayvec::ArrayVec;
    use common::TempDir;

    use crate::combine::frame_cache::frame_chunk::{ChunkCopy, FrameChunk};
    use crate::frame::frame_quality::{DropPlanes, FrameQuality};
    use crate::frame::frame_stats::FrameStats;
    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_frame::StoredFrame;
    use crate::image::pixel_flags::PixelFlags;
    use crate::image::quality_flags::QualityFlags;
    use crate::internals::prelude::*;

    /// A copied chunk reads what the planes hold there, plane for plane, for a frame with warp
    /// planes and flags and for a drizzled one with drops per channel, spilled and resident alike:
    /// six pixels of three channels, the chunk pixels 2 to 5.
    #[test]
    fn a_copied_chunk_reads_as_the_planes_do() {
        let directory = TempDir::new("frame_chunk_copy");
        let scratch = RunScratch::create(directory.path()).unwrap();
        let dimensions = ImageDimensions::new((6, 1), 3);
        let ramp = |offset: f32| -> Vec<f32> { (0..6).map(|x| offset + x as f32).collect() };
        let mut image =
            LinearImage::from_planar_channels(dimensions, [ramp(0.0), ramp(10.0), ramp(20.0)]);
        image.flags = PixelFlags::from_fn(dimensions.size(), |index| {
            if index == 3 {
                QualityFlags::SATURATED
            } else {
                QualityFlags::default()
            }
        });
        let warped = FrameQuality::Planes {
            coverage: Buffer2::new(6, 1, ramp(0.5)),
            confidence: Buffer2::new(6, 1, ramp(0.25)),
        };
        let drops: ArrayVec<DropPlanes<Buffer2<f32>>, 3> = (0..3)
            .map(|channel| DropPlanes {
                weight: Buffer2::new(6, 1, ramp(channel as f32)),
                confidence: Buffer2::new(6, 1, ramp(5.0 + channel as f32)),
            })
            .collect();
        let drizzled = FrameQuality::Drizzled { drops };
        let stats = FrameStats::measure(&image);
        let frames = vec![
            StoredFrame::spill(&scratch, &image, &warped, stats.clone()).unwrap(),
            StoredFrame::spill(&scratch, &image, &drizzled, stats.clone()).unwrap(),
            StoredFrame::from_memory(image.clone(), drizzled.clone(), stats),
        ];
        let mut copy = ChunkCopy::new(&frames, 3);
        copy.fill(&frames, 2..5);
        for (frame, copied) in frames.iter().zip(copy.views(&frames)) {
            let borrowed = FrameChunk::borrowed(frame, 2..5);
            assert_eq!(copied.channels, borrowed.channels);
            assert_eq!(copied.flags, borrowed.flags);
            assert_eq!(
                copied.flags,
                Some(&[0, QualityFlags::SATURATED.byte(), 0][..])
            );
            let planes = |chunk: &FrameChunk<'_>| -> Vec<Vec<f32>> {
                chunk
                    .quality
                    .present()
                    .map(|(_, plane)| plane.to_vec())
                    .collect()
            };
            assert_eq!(planes(&copied), planes(&borrowed));
            assert_eq!(copied.channels[1], [12.0, 13.0, 14.0]);
        }
    }
}
