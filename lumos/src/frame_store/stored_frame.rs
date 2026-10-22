//! One frame as the combine engine sees it.

use std::borrow::Borrow;
use std::fmt::Debug;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame::admitted::Admitted;
use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_quality::FrameQuality;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_plane::StoredPlane;
use crate::frame_store::stratified_samples::StratifiedSamples;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::stackable_image::{ImageParts, StackableImage};

/// One frame as the combine engine sees it: its channel planes, the per-pixel quality it carries
/// if a warp produced one or its source declared pixels with no measurement, its flags when its
/// quality is a mask of them or it carries any but `NO_DATA` (which quality planes already hold),
/// the gain a flat applied to its pixels, and the statistics measured on the source before any
/// interpolation.
#[derive(Debug)]
pub(crate) struct StoredFrame {
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    /// Each slot's [`StratifiedSamples`], gathered as a frame of no quality went to disk, where
    /// reading them from its planes would read every page; `None` for a frame in memory, or one
    /// whose quality makes normalization sample the pixels the frames share instead.
    pub(crate) samples: Option<ArrayVec<StoredPlane, 3>>,
    pub(crate) quality: FrameQuality<StoredPlane>,
    pub(crate) flags: Option<StoredPlane<u8>>,
    pub(crate) flat_gain: Option<StoredGain>,
    pub(crate) source_stats: FrameStats,
    pub(crate) facts: FrameFacts,
}

impl StoredFrame {
    pub(crate) fn from_memory(
        image: impl StackableImage,
        quality: FrameQuality<Buffer2<f32>>,
        admitted: Admitted,
    ) -> Self {
        let Admitted { stats, facts } = admitted;
        let ImageParts {
            planes,
            flags,
            flat_gain,
        } = image.into_parts();
        let flat_gain = flat_gain.map(StoredGain::Resident);
        let flags = flags
            .filter(|flags| quality.keeps(flags))
            .map(|flags| StoredPlane::Memory(flags.into_buffer()));
        Self {
            channels: planes.into_iter().map(StoredPlane::Memory).collect(),
            samples: None,
            quality: quality.map(StoredPlane::Memory),
            flags,
            flat_gain,
            source_stats: stats,
            facts,
        }
    }

    /// Each slot's [`StratifiedSamples`] of `image`, each stored by `store(slot, samples)`.
    pub(super) fn store_samples(
        image: &impl StackableImage,
        mut store: impl FnMut(usize, &[f32]) -> Result<StoredPlane, FrameStoreError>,
    ) -> Result<ArrayVec<StoredPlane, 3>, FrameStoreError> {
        let size = image.dimensions().size();
        let mosaic = image.cfa_type().filter(CfaType::is_mosaic);
        let slots = mosaic.map_or_else(|| image.dimensions().channels(), |cfa| cfa.color_count());
        (0..slots)
            .map(|slot| {
                let plane = image.channel(if mosaic.is_some() { 0 } else { slot });
                let samples = StratifiedSamples::new(size, mosaic.as_ref(), slot).gather(plane);
                store(slot, &samples)
            })
            .collect()
    }
}

/// A frame on its way to a run's scratch: its image, its quality planes, and what its admission
/// found of its source.
#[derive(Debug)]
pub(crate) struct SpillFrame<'a, I, Q> {
    pub(crate) image: &'a I,
    pub(crate) quality: &'a FrameQuality<Q>,
    pub(crate) admitted: Admitted,
}

impl<I: StackableImage, Q: Borrow<Buffer2<f32>>> SpillFrame<'_, I, Q> {
    /// Write the frame's channels, quality planes, flags and gain to `scratch` and memory-map them
    /// back.
    ///
    /// Borrows everything it writes: the caller keeps its buffers, which is what lets the warp
    /// stage hand the same ones to the next frame rather than allocating a set that has to be
    /// faulted in from scratch.
    pub(crate) fn spill(self, scratch: &RunScratch) -> Result<StoredFrame, FrameStoreError> {
        let Self {
            image,
            quality,
            admitted: Admitted { stats, facts },
        } = self;
        let channels = (0..image.dimensions().channels())
            .map(|channel| scratch.store(image.channel(channel)))
            .collect::<Result<_, _>>()?;
        let samples = quality
            .is_none()
            .then(|| StoredFrame::store_samples(image, |_, samples| scratch.store(samples)))
            .transpose()?;
        let quality = quality.try_map(|_, buffer| scratch.store(buffer.borrow().pixels()))?;
        let flags = image
            .flags()
            .filter(|flags| quality.keeps(flags))
            .map(|flags| scratch.store(flags.bytes()))
            .transpose()?;
        let flat_gain = image
            .flat_gain()
            .map(|gain| StoredGain::spill(scratch, gain))
            .transpose()?;
        Ok(StoredFrame {
            channels,
            samples,
            quality,
            flags,
            flat_gain,
            source_stats: stats,
            facts,
        })
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use arrayvec::ArrayVec;

    use crate::frame::frame_quality::FrameQuality;
    use crate::frame_store::stored_frame::StoredFrame;
    use crate::frame_store::stored_gain::StoredGain;
    use crate::frame_store::stored_plane::StoredPlane;

    impl StoredFrame {
        /// Every `f32` plane the frame holds: its channels, its samples, its quality planes and
        /// its flat gain's.
        pub(crate) fn planes(&self) -> impl Iterator<Item = &StoredPlane> {
            let quality: ArrayVec<&StoredPlane, 6> = match &self.quality {
                FrameQuality::None | FrameQuality::Mask { .. } => ArrayVec::new(),
                FrameQuality::Planes {
                    coverage,
                    confidence,
                } => [coverage, confidence].into_iter().collect(),
                FrameQuality::Drizzled { drops } => drops
                    .iter()
                    .flat_map(|drop| [&drop.weight, &drop.confidence])
                    .collect(),
            };
            self.channels
                .iter()
                .chain(self.samples.iter().flatten())
                .chain(quality)
                .chain(self.flat_gain.iter().flat_map(StoredGain::planes))
        }

        /// Drop every page of the frame from the page cache, its flags' too, so the combine's next
        /// read of it comes from the disk; whether each plane went, which a resident one does not.
        pub(crate) fn evict(&self) -> bool {
            let flags = self.flags.as_ref().is_none_or(StoredPlane::evict);
            self.planes().fold(flags, |all, plane| plane.evict() & all)
        }
    }
}

#[cfg(test)]
mod tests {
    use common::{MappedFile, TempDir};
    use imaginarium::Buffer2;

    use crate::frame::admitted::Admitted;
    use crate::frame::frame_quality::FrameQuality;
    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_frame::SpillFrame;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::linear_image::LinearImage;
    use crate::math::size2us::Size2us;

    /// A spilled frame with quality planes leaves the page cache whole where the system evicts:
    /// its three channels and its coverage and confidence, 512² × 4 bytes = 1 MiB each, every page
    /// out by the `mincore` count each plane's eviction ends with, and its channels read back as
    /// written.
    #[test]
    fn an_evicted_frame_leaves_every_plane_on_the_disk() {
        let dir = TempDir::new("stored_frame_evict");
        let scratch = RunScratch::create_evictable(dir.path()).unwrap();
        let size = Size2us::new(512, 512);
        let image = LinearImage::from_planar_channels(
            ImageDimensions::new(size, ChannelLayout::Rgb),
            [0.25f32, 0.5, 0.75].map(|level| vec![level; size.pixel_count()]),
        );
        let plane = |level| Buffer2::new_filled(size.width, size.height, level);
        let quality = FrameQuality::Planes {
            coverage: plane(1.0),
            confidence: plane(0.5),
        };
        let stats = Admitted::measure(&image);
        let frame = SpillFrame {
            image: &image,
            quality: &quality,
            admitted: stats,
        }
        .spill(&scratch)
        .unwrap();
        assert_eq!(frame.planes().count(), 5);
        assert_eq!(frame.evict(), MappedFile::EVICTS);
        for (channel, level) in frame.channels.iter().zip([0.25, 0.5, 0.75]) {
            let read = channel.chunk(0..size.pixel_count());
            assert!(read.iter().all(|&value| value == level));
        }
    }
}
