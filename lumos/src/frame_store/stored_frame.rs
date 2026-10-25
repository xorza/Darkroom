//! One frame as the combine engine sees it, and the writer every frame a sink holds goes through.

use arrayvec::ArrayVec;
use common::Buffer2;

use crate::frame::admitted::Admitted;
use crate::frame::frame_facts::FrameFacts;
use crate::frame::frame_quality::FrameQuality;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::plane_sink::{PlaneRole, PlaneSink};
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_image::StoredImage;
use crate::frame_store::stored_plane::StoredPlane;
use crate::frame_store::stratified_samples::StratifiedSamples;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::stackable_image::{ImageParts, StackableImage};

/// One frame as the combine engine sees it: its channel planes, the per-pixel quality it carries
/// if a warp produced one or its source declared pixels with no measurement, its flags when its
/// quality [keeps](FrameQuality::keeps) them, the gain a flat applied to its pixels, and the
/// statistics measured on the source before any interpolation.
#[derive(Debug)]
pub(crate) struct StoredFrame {
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    /// Each slot's [`StratifiedSamples`], gathered as a frame of no quality in a set of
    /// [`SetKind::Unwarped`] went to disk, where reading them from its planes would read every
    /// page; `None` for a frame in memory, or one whose quality or set makes normalization sample
    /// the pixels the frames share instead.
    pub(crate) samples: Option<ArrayVec<StoredPlane, 3>>,
    pub(crate) quality: FrameQuality<StoredPlane>,
    pub(crate) flags: Option<StoredPlane<u8>>,
    pub(crate) flat_gain: Option<StoredGain>,
    pub(crate) source_stats: FrameStats,
    pub(crate) facts: FrameFacts,
}

impl StoredFrame {
    /// `image` with `quality` in RAM, as the resident tier keeps it: its buffers moved in.
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
        let flags = flags
            .filter(|flags| quality.keeps(flags.counts()))
            .map(|flags| StoredPlane::Memory(flags.into_buffer()));
        Self {
            channels: planes.into_iter().map(StoredPlane::Memory).collect(),
            samples: None,
            quality: quality.map(StoredPlane::Memory),
            flags,
            flat_gain: flat_gain.map(StoredGain::from),
            source_stats: stats,
            facts,
        }
    }
}

/// How the frames of a set cover its pixels, which decides whether a frame of no quality gathers
/// its own samples as it is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetKind {
    /// Frames nothing warped, each of which may cover every pixel: normalization then samples
    /// each slot at the same stratified pixels of every frame.
    Unwarped,
    /// Frames registered onto one grid, by a warp or a drizzle, each covering a part of it:
    /// normalization samples the pixels they share, and reads no frame's own samples, the
    /// reference's among them.
    Registered,
}

/// A frame on its way to a [`PlaneSink`]: its image and its quality planes, borrowed, and the kind
/// of set it is of.
#[derive(Debug)]
pub(crate) struct FrameWrite<'a, I> {
    pub(crate) image: &'a I,
    pub(crate) quality: FrameQuality<&'a Buffer2<f32>>,
    pub(crate) set: SetKind,
}

/// The planes a [`FrameWrite`] wrote: the frame they make once its admission joins them.
#[derive(Debug)]
pub(crate) struct WrittenFrame {
    pub(crate) image: StoredImage,
    pub(crate) samples: Option<ArrayVec<StoredPlane, 3>>,
    pub(crate) quality: FrameQuality<StoredPlane>,
}

impl<I: StackableImage> FrameWrite<'_, I> {
    /// Write the planes the frame keeps to `sink` and map them back: its channels, its flags when
    /// its quality [keeps](FrameQuality::keeps) them, each slot's samples when it has no quality
    /// in a set of [`SetKind::Unwarped`], its quality planes and its own flat gain; a shared gain
    /// stays the handle the run holds.
    ///
    /// Borrows everything it writes: the caller keeps its buffers, which is what lets a producer
    /// fill the same ones for its next frame rather than a fresh set that has to be faulted in.
    pub(crate) fn write(self, mut sink: impl PlaneSink) -> Result<WrittenFrame, FrameStoreError> {
        let Self {
            image,
            quality,
            set,
        } = self;
        let flags = image.flags().filter(|flags| quality.keeps(flags.counts()));
        let stored = StoredImage::write(image, flags, &mut sink)?;
        let samples = (quality.is_none() && set == SetKind::Unwarped)
            .then(|| store_samples(image, &mut sink))
            .transpose()?;
        let quality = quality.try_map(|_, plane| sink.write(PlaneRole::Quality, plane.pixels()))?;
        Ok(WrittenFrame {
            image: stored,
            samples,
            quality,
        })
    }
}

impl WrittenFrame {
    /// The frame the planes make, as its admission found it.
    pub(crate) fn into_frame(self, admitted: Admitted) -> StoredFrame {
        let Self {
            image,
            samples,
            quality,
        } = self;
        let Admitted { stats, facts } = admitted;
        let flags = image
            .flags
            .filter(|flags| quality.keeps(&flags.counts))
            .map(|flags| flags.plane);
        StoredFrame {
            channels: image.channels,
            samples,
            quality,
            flags,
            flat_gain: image.flat_gain,
            source_stats: stats,
            facts,
        }
    }
}

/// Each slot's [`StratifiedSamples`] of `image`, written to `sink` from one buffer the slots take
/// in turn.
fn store_samples(
    image: &impl StackableImage,
    mut sink: impl PlaneSink,
) -> Result<ArrayVec<StoredPlane, 3>, FrameStoreError> {
    let size = image.dimensions().size();
    let mosaic = image.cfa_type().and_then(CfaType::mosaic);
    let slots = mosaic.map_or_else(
        || image.dimensions().channels(),
        |mosaic| mosaic.color_count(),
    );
    let mut samples = Vec::new();
    (0..slots)
        .map(|slot| {
            let plane = image.channel(if mosaic.is_some() { 0 } else { slot });
            StratifiedSamples::new(size, mosaic, slot).gather(plane, &mut samples);
            sink.write(PlaneRole::Samples(slot), &samples)
        })
        .collect()
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
    use common::{Buffer2, MappedFile, TempDir};

    use crate::frame::admitted::Admitted;
    use crate::frame::frame_quality::FrameQuality;
    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_frame::{FrameWrite, SetKind};
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
        let frame = FrameWrite {
            image: &image,
            quality: quality.as_ref(),
            set: SetKind::Unwarped,
        }
        .write(&scratch)
        .unwrap()
        .into_frame(Admitted::measure(&image));
        assert_eq!(frame.planes().count(), 5);
        assert_eq!(frame.evict(), MappedFile::EVICTS);
        for (channel, level) in frame.channels.iter().zip([0.25, 0.5, 0.75]) {
            let read = channel.chunk(0..size.pixel_count());
            assert!(read.iter().all(|&value| value == level));
        }
    }
}
