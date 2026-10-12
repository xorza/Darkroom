//! A calibrated image stored between detection and registration.

use std::fmt::Debug;

use arrayvec::ArrayVec;

use crate::frame::frame_quality::FrameQuality;
use crate::frame::frame_stats::FrameStats;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::plane_store::PlaneStore;
use crate::frame_store::stored_frame::StoredFrame;
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_plane::StoredPlane;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::linear_image::LinearImage;
use crate::image::pixel_flags::PixelFlags;
use crate::image::quality_flags::QualityFlags;

/// A calibrated image stored on disk between detection and registration.
#[derive(Debug)]
pub(crate) struct StoredImage {
    pub(crate) metadata: ImageMetadata,
    pub(crate) dimensions: ImageDimensions,
    channels: ArrayVec<StoredPlane, 3>,
    /// The image's [`PixelFlags`] bytes, for an image that carries any. Spilled with the channels:
    /// without them the fill under every null reads back as a measurement.
    flags: Option<StoredPlane<u8>>,
}

impl StoredImage {
    /// Write `image`'s channels and flags to `store` and memory-map them back.
    pub(crate) fn spill(
        store: &impl PlaneStore,
        image: &LinearImage,
    ) -> Result<Self, FrameStoreError> {
        let flags = image
            .flags
            .as_ref()
            .map(|flags| store.store_flags(flags.bytes()))
            .transpose()?;
        let channels = (0..image.channels())
            .map(|channel| store.store_channel(channel, image.channel(channel).pixels()))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            metadata: image.metadata.clone(),
            dimensions: image.dimensions(),
            channels,
            flags,
        })
    }

    /// Each channel's samples, read in place from the map.
    pub(crate) fn planes(&self) -> impl Iterator<Item = &[f32]> {
        let samples = self.dimensions.pixel_count();
        self.channels
            .iter()
            .map(move |plane| plane.chunk(0, samples))
    }

    /// The image read back into RAM: its channels, flags and metadata, each plane copied out of
    /// its map front to back.
    pub(crate) fn load(&self) -> LinearImage {
        let mut image = LinearImage::from_planar_channels(
            self.dimensions,
            self.planes().map(<[f32]>::to_vec).collect::<Vec<_>>(),
        );
        image.metadata = self.metadata.clone();
        image.flags = self.flags();
        image
    }

    /// The image's flags, copied out of the map: a byte a pixel, against the four a channel
    /// costs.
    pub(crate) fn flags(&self) -> Option<PixelFlags> {
        self.flags.as_ref().map(|plane| {
            PixelFlags::from_bytes(
                self.dimensions.size(),
                plane.chunk(0, self.dimensions.pixel_count()),
            )
        })
    }

    /// The image as the reference frame of a registered stack, its channels and flags the planes
    /// already on disk — see [`FrameQuality::for_reference`]. Its flags plane stays for that mask or
    /// when it holds a flag the warp carries.
    pub(crate) fn into_reference_frame(
        self,
        store: &impl PlaneStore,
        source_stats: FrameStats,
    ) -> Result<StoredFrame, FrameStoreError> {
        let flags = self.flags();
        let quality = FrameQuality::for_reference(flags.as_ref())
            .try_map(|plane, buffer| store.store_quality(plane, buffer.pixels()))?;
        let keeps = flags.as_ref().is_some_and(|flags| {
            quality.mask().is_some() || flags.contains(QualityFlags::RESAMPLE_CARRIED)
        });
        let flat_gain = self
            .metadata
            .flat_gain
            .as_deref()
            .map(|gain| StoredGain::spill(store, gain))
            .transpose()?;
        Ok(StoredFrame {
            channels: self.channels,
            samples: None,
            quality,
            flags: self.flags.filter(|_| keeps),
            flat_gain,
            source_stats,
        })
    }
}

#[cfg(any(test, feature = "bench"))]
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
    use common::TempDir;

    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_image::StoredImage;
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
        let scratch = RunScratch::create(directory.path()).unwrap();
        let size = Size2us::new(512, 512);
        let mut image = LinearImage::from_planar_channels(
            ImageDimensions::new(size, 3),
            [0.25f32, 0.5, 0.75].map(|level| vec![level; size.pixel_count()]),
        );
        let bytes: Vec<u8> = (0..size.pixel_count())
            .map(|index| (index % 2) as u8)
            .collect();
        image.flags = Some(PixelFlags::from_bytes(size, &bytes));
        let parked = StoredImage::spill(&scratch, &image).unwrap();
        assert_eq!(parked.evict(), MappedFile::EVICTS);
        let loaded = parked.load();
        for channel in 0..3 {
            assert_eq!(
                loaded.channel(channel).pixels(),
                image.channel(channel).pixels()
            );
        }
        assert_eq!(loaded.flags.unwrap().bytes(), bytes);
    }
}
