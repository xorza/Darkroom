//! [`FrameChunk`]: one frame over one chunk of rows, as the combine reads it.

use std::convert::Infallible;
use std::mem;
use std::ops::Range;

use arrayvec::ArrayVec;
use common::thread;

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
        Self {
            channels: frame
                .channels
                .iter()
                .map(|plane| plane.chunk(range.clone()))
                .collect(),
            quality: frame
                .quality
                .try_map(|_, plane| Ok::<_, Infallible>(plane.chunk(range.clone())))
                .unwrap_or_else(|never| match never {}),
            flags: frame.flags.as_ref().map(|plane| plane.chunk(range.clone())),
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
///
/// One buffer of samples and one of flag bytes, each plane `stride` long: a frame's channels and
/// then its quality planes in [`FrameQuality::present`] order take consecutive planes of the
/// samples, frame after frame, and each frame with flags a plane of the flag bytes.
#[derive(Debug)]
pub(crate) struct ChunkCopy {
    samples: Vec<f32>,
    flags: Vec<u8>,
    frames: Vec<FramePlanes>,
    /// The pixels a plane has room for: the largest chunk.
    stride: usize,
    /// The pixels the last [`Self::fill`] copied into each plane.
    len: usize,
}

/// Where one frame's planes lie in a [`ChunkCopy`]: its planes of samples, and its plane of flag
/// bytes when it carries flags.
#[derive(Debug, Clone)]
struct FramePlanes {
    samples: Range<usize>,
    flags: Option<usize>,
}

impl ChunkCopy {
    /// Buffers for `frames`' planes, each sized for a chunk of `pixels` and filled by
    /// [`Self::fill`].
    pub(crate) fn new(frames: &[StoredFrame], pixels: usize) -> Self {
        let (mut sample_planes, mut flag_planes) = (0, 0);
        let frames: Vec<FramePlanes> = frames
            .iter()
            .map(|frame| {
                let start = sample_planes;
                sample_planes += frame.channels.len() + frame.quality.present().count();
                let flags = frame.flags.as_ref().map(|_| {
                    flag_planes += 1;
                    flag_planes - 1
                });
                FramePlanes {
                    samples: start..sample_planes,
                    flags,
                }
            })
            .collect();
        Self {
            samples: vec![0.0; sample_planes * pixels],
            flags: vec![0; flag_planes * pixels],
            frames,
            stride: pixels,
            len: 0,
        }
    }

    /// Copy `frames`' pixels `range` into the buffers, plane by plane, on [`READERS`] threads of
    /// its own, each taking a run of the frames and the disjoint run of planes they own.
    pub(crate) fn fill(&mut self, frames: &[StoredFrame], range: Range<usize>) {
        debug_assert_eq!(frames.len(), self.frames.len());
        debug_assert!(range.len() <= self.stride, "a chunk fits its planes");
        self.len = range.len();
        let stride = self.stride;
        let per_reader = frames.len().div_ceil(READERS).max(1);
        let (mut samples, mut flags) = (self.samples.as_mut_slice(), self.flags.as_mut_slice());
        thread::scope(|scope| {
            for (frames, planes) in frames
                .chunks(per_reader)
                .zip(self.frames.chunks(per_reader))
            {
                let sample_planes = planes
                    .iter()
                    .map(|planes| planes.samples.len())
                    .sum::<usize>();
                let flag_planes = planes
                    .iter()
                    .filter(|planes| planes.flags.is_some())
                    .count();
                let (own_samples, rest) =
                    mem::take(&mut samples).split_at_mut(sample_planes * stride);
                samples = rest;
                let (own_flags, rest) = mem::take(&mut flags).split_at_mut(flag_planes * stride);
                flags = rest;
                let range = range.clone();
                scope.spawn("lumos-frame-reader", move || {
                    Self::fill_frames(frames, own_samples, own_flags, stride, range);
                });
            }
        });
    }

    /// Copy each of `frames`' planes over `range` into the next plane of `samples` or `flags`,
    /// `stride` apart.
    fn fill_frames(
        frames: &[StoredFrame],
        samples: &mut [f32],
        flags: &mut [u8],
        stride: usize,
        range: Range<usize>,
    ) {
        let mut sample_planes = samples.chunks_mut(stride);
        let mut flag_planes = flags.chunks_mut(stride);
        for frame in frames {
            let quality = frame.quality.present().map(|(_, plane)| plane);
            for plane in frame.channels.iter().chain(quality) {
                let buffer = sample_planes
                    .next()
                    .expect("a plane for every sample plane");
                buffer[..range.len()].copy_from_slice(plane.chunk(range.clone()));
            }
            if let Some(plane) = &frame.flags {
                let buffer = flag_planes.next().expect("a plane for every flag plane");
                buffer[..range.len()].copy_from_slice(plane.chunk(range.clone()));
            }
        }
    }

    /// Into `out`, cleared first, each frame's chunk as the last [`Self::fill`] left it, shaped
    /// as `frames` are.
    pub(crate) fn views_into<'a>(&'a self, frames: &[StoredFrame], out: &mut Vec<FrameChunk<'a>>) {
        let Self {
            samples,
            flags,
            stride,
            len,
            ..
        } = self;
        let plane = |idx: usize| &samples[idx * stride..][..*len];
        out.clear();
        out.extend(frames.iter().zip(&self.frames).map(|(frame, planes)| {
            let channels = frame.channels.len();
            let mut quality = planes.samples.start + channels..planes.samples.end;
            FrameChunk {
                channels: (planes.samples.start..planes.samples.start + channels)
                    .map(plane)
                    .collect(),
                quality: frame
                    .quality
                    .try_map(|_, _| {
                        Ok::<_, Infallible>(plane(
                            quality.next().expect("a plane for every quality plane"),
                        ))
                    })
                    .unwrap_or_else(|never| match never {}),
                flags: planes.flags.map(|idx| &flags[idx * stride..][..*len]),
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use arrayvec::ArrayVec;
    use common::{Buffer2, TempDir};
    use imaginarium::{ChannelLayout, PlanarDesc};

    use crate::combine::frame_read::frame_chunk::{ChunkCopy, FrameChunk};
    use crate::frame::admitted::Admitted;
    use crate::frame::frame_quality::{DropPlanes, FrameQuality};
    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_frame::{FrameWrite, SetKind, StoredFrame};
    use crate::image::flags::pixel_flags::PixelFlags;
    use crate::image::flags::quality_flags::QualityFlags;
    use crate::image::linear_image::LinearImage;

    /// A copied chunk reads what the planes hold there, plane for plane, for a frame with warp
    /// planes and flags and for a drizzled one with drops per channel, spilled and resident alike:
    /// six pixels of three channels, the chunk pixels 2 to 5, then the last pixel alone.
    #[test]
    fn a_copied_chunk_reads_as_the_planes_do() {
        let dir = TempDir::new("frame_chunk_copy");
        let scratch = RunScratch::create(dir.path()).unwrap();
        let desc = PlanarDesc::new((6, 1), ChannelLayout::Rgb);
        let ramp = |offset: f32| -> Vec<f32> { (0..6).map(|x| offset + x as f32).collect() };
        let mut image =
            LinearImage::from_planar_channels(desc, [ramp(0.0), ramp(10.0), ramp(20.0)]);
        image.set_flags(PixelFlags::from_fn(desc.size(), |idx| {
            if idx == 3 {
                QualityFlags::SATURATED
            } else {
                QualityFlags::default()
            }
        }));
        let warped = FrameQuality::Planes {
            coverage: Buffer2::new((6, 1), ramp(0.5)),
            confidence: Buffer2::new((6, 1), ramp(0.25)),
        };
        let drops: ArrayVec<DropPlanes<Buffer2<f32>>, 3> = (0..3)
            .map(|channel| DropPlanes {
                weight: Buffer2::new((6, 1), ramp(channel as f32)),
                confidence: Buffer2::new((6, 1), ramp(5.0 + channel as f32)),
            })
            .collect();
        let drizzled = FrameQuality::Drizzled { drops };
        let stats = Admitted::measure(&image);
        let frames = vec![
            FrameWrite {
                image: &image,
                quality: warped.as_ref(),
                set: SetKind::Registered,
            }
            .write(&scratch)
            .unwrap()
            .into_frame(stats.clone()),
            FrameWrite {
                image: &image,
                quality: drizzled.as_ref(),
                set: SetKind::Registered,
            }
            .write(&scratch)
            .unwrap()
            .into_frame(stats.clone()),
            StoredFrame::from_memory(image.clone(), drizzled, stats),
        ];
        let mut copy = ChunkCopy::new(&frames, 3);
        copy.fill(&frames, 2..5);
        let mut views = Vec::new();
        copy.views_into(&frames, &mut views);
        for (frame, copied) in frames.iter().zip(views) {
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

        // The last chunk is shorter than the planes' room, one pixel: each plane reads it alone.
        copy.fill(&frames, 5..6);
        let mut last = Vec::new();
        copy.views_into(&frames, &mut last);
        for (frame, copied) in frames.iter().zip(&last) {
            let borrowed = FrameChunk::borrowed(frame, 5..6);
            assert_eq!(copied.channels, borrowed.channels);
            assert_eq!(copied.flags, borrowed.flags);
            assert_eq!(copied.channels[2], [25.0]);
        }
    }
}
