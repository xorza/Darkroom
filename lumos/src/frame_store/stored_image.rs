//! [`StoredImage`]: an image whose planes are memory maps, between the stages that read it.

use std::fmt::Debug;
use std::sync::Arc;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_plane::StoredPlane;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::FlatGain;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::linear_image::LinearImage;
use crate::image::pixel_flags::PixelFlags;
use crate::image::stackable_image::{ImageParts, StackableImage};

/// An image whose channels and flags are memory maps: a calibrated light parked between detection
/// and registration, or a kept frame read back from the decode cache.
#[derive(Debug)]
pub(crate) struct StoredImage {
    pub(crate) metadata: ImageMetadata,
    pub(crate) dimensions: ImageDimensions,
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    /// The image's [`PixelFlags`] bytes, for an image that carries any. Stored with the channels:
    /// without them the fill under every null reads back as a measurement.
    pub(crate) flags: Option<StoredPlane<u8>>,
    /// The gain a flat applied to the image's pixels, for an image a flat divided.
    pub(crate) flat_gain: Option<Arc<FlatGain>>,
}

impl StoredImage {
    /// Write `image`'s channels and flags to `scratch` and memory-map them back.
    pub(crate) fn spill(
        scratch: &RunScratch,
        image: &LinearImage,
    ) -> Result<Self, FrameStoreError> {
        let flags = image
            .flags()
            .map(|flags| scratch.store(flags.bytes()))
            .transpose()?;
        let channels = (0..image.channels())
            .map(|channel| scratch.store(image.channel(channel).pixels()))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            metadata: image.metadata.clone(),
            dimensions: image.dimensions(),
            channels,
            flags,
            flat_gain: image.flat_gain().cloned(),
        })
    }

    /// Each channel's samples, read in place from the map.
    pub(crate) fn planes(&self) -> impl Iterator<Item = &[f32]> {
        let samples = self.dimensions.pixel_count();
        self.channels
            .iter()
            .map(move |plane| plane.chunk(0..samples))
    }

    /// The image read back into RAM, of `cfa_type`: its channels, flags and metadata, each plane
    /// copied out of its map front to back.
    pub(crate) fn into_image<I: StackableImage>(self, cfa_type: Option<CfaType>) -> I {
        let size = self.dimensions.size();
        let parts = ImageParts {
            planes: self
                .planes()
                .map(|plane| Buffer2::new(size.width, size.height, plane.to_vec()))
                .collect(),
            flags: self.flags(),
            flat_gain: self.flat_gain,
        };
        I::from_parts(parts, cfa_type, self.metadata)
    }

    /// The image's flags, copied out of the map: a byte a pixel, against the four a channel
    /// costs.
    pub(crate) fn flags(&self) -> Option<PixelFlags> {
        let size = self.dimensions.size();
        let plane = self.flags.as_ref()?;
        PixelFlags::from_bytes(Buffer2::new(
            size.width,
            size.height,
            plane.chunk(0..size.pixel_count()).to_vec(),
        ))
    }

    /// The image as the reference frame of a registered stack, its channels and flags the planes
    /// already on disk — see [`FrameQuality::for_reference`] — and its quality planes and gain
    /// written to `scratch`.
    pub(crate) fn into_reference_frame(
        self,
        scratch: &RunScratch,
        admitted: Admitted,
    ) -> Result<StoredFrame, FrameStoreError> {
        let Admitted { stats, facts } = admitted;
        let flags = self.flags();
        let quality = FrameQuality::for_reference(flags.as_ref())
            .try_map(|_, buffer| scratch.store(buffer.pixels()))?;
        let keeps = flags.as_ref().is_some_and(|flags| quality.keeps(flags));
        let flat_gain = self
            .flat_gain
            .as_deref()
            .map(|gain| StoredGain::spill(scratch, gain))
            .transpose()?;
        Ok(StoredFrame {
            channels: self.channels,
            samples: None,
            quality,
            flags: self.flags.filter(|_| keeps),
            flat_gain,
            source_stats: stats,
            facts,
        })
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::frame_store::stored_image::StoredImage;
    use crate::frame_store::stored_plane::StoredPlane;

    impl StoredImage {
        /// Drop the image's pages from the page cache, its channels' and its flags', so its next
        /// read comes from the disk; whether each plane went, which a resident one does not.
        pub(crate) fn evict(&self) -> bool {
            let flags = self.flags.as_ref().is_none_or(StoredPlane::evict);
            self.channels
                .iter()
                .fold(flags, |all, plane| plane.evict() & all)
        }
    }
}

#[cfg(test)]
mod tests {
    use imaginarium::Buffer2;

    use common::TempDir;

    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_image::StoredImage;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::linear_image::LinearImage;
    use crate::image::pixel_flags::PixelFlags;
    use crate::math::size2us::Size2us;
    use crate::platform::MappedFile;

    /// A parked image leaves the page cache whole where the system evicts, its flags with its
    /// channels, and loads back as it was spilled.
    #[test]
    fn an_evicted_image_leaves_every_plane_on_the_disk() {
        let directory = TempDir::new("stored_image_evict");
        let scratch = RunScratch::create_evictable(directory.path()).unwrap();
        let size = Size2us::new(512, 512);
        let mut image = LinearImage::from_planar_channels(
            ImageDimensions::new(size, ChannelLayout::Rgb),
            [0.25f32, 0.5, 0.75].map(|level| vec![level; size.pixel_count()]),
        );
        let bytes: Vec<u8> = (0..size.pixel_count())
            .map(|index| (index % 2) as u8)
            .collect();
        image.set_flags(PixelFlags::from_bytes(Buffer2::new(
            size.width,
            size.height,
            bytes.clone(),
        )));
        let parked = StoredImage::spill(&scratch, &image).unwrap();
        assert_eq!(parked.evict(), MappedFile::EVICTS);
        let loaded: LinearImage = parked.into_image(None);
        for channel in 0..3 {
            assert_eq!(
                loaded.channel(channel).pixels(),
                image.channel(channel).pixels()
            );
        }
        assert_eq!(loaded.flags().unwrap().bytes(), bytes);
    }
}
