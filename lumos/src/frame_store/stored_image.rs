//! [`StoredImage`]: an image whose planes are memory maps, between the stages that read it.

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame::admitted::Admitted;
use crate::frame::frame_quality::FrameQuality;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::plane_sink::{PlaneRole, PlaneSink};
use crate::frame_store::stored_frame::{StoredFrame, WrittenFrame};
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_plane::StoredPlane;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flag_counts::FlagCounts;
use crate::image::image_dimensions::ImageDimensions;
use crate::image::image_metadata::ImageMetadata;
use crate::image::pixel_flags::PixelFlags;
use crate::image::stackable_image::{ImageParts, StackableImage};

/// An image whose channels, flags and flat gain are memory maps: a calibrated light parked between
/// detection and registration, or a frame the decode cache keeps.
#[derive(Debug)]
pub(crate) struct StoredImage {
    pub(crate) metadata: ImageMetadata,
    pub(crate) dimensions: ImageDimensions,
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    /// The image's [`PixelFlags`], for an image that carries any and keeps them. Stored with the
    /// channels: without them the fill under every null reads back as a measurement.
    pub(crate) flags: Option<StoredFlags>,
    /// The gain a flat applied to the image's pixels, for an image a flat divided.
    pub(crate) flat_gain: Option<StoredGain>,
}

/// A stored image's flag bytes, and how many pixels hold each flag, counted before they were
/// written: what a frame keeps of them is decided without a read of the plane.
#[derive(Debug)]
pub(crate) struct StoredFlags {
    pub(crate) plane: StoredPlane<u8>,
    pub(crate) counts: FlagCounts,
}

impl StoredImage {
    /// `image`'s channels, `flags` and flat gain written to `sink` and mapped back: `flags` are
    /// the image's own, or none when the frame does not keep them.
    pub(crate) fn write<I: StackableImage>(
        image: &I,
        flags: Option<&PixelFlags>,
        mut sink: impl PlaneSink,
    ) -> Result<Self, FrameStoreError> {
        let dimensions = image.dimensions();
        let channels = (0..dimensions.channels())
            .map(|channel| sink.write(PlaneRole::Channel(channel), image.channel(channel).pixels()))
            .collect::<Result<_, _>>()?;
        let flags = match flags {
            Some(flags) => Some(StoredFlags {
                plane: sink.write(PlaneRole::Flags, flags.bytes())?,
                counts: *flags.counts(),
            }),
            None => None,
        };
        let flat_gain = image
            .flat_gain()
            .map(|gain| StoredGain::write(gain, &mut sink))
            .transpose()?;
        Ok(Self {
            metadata: image.metadata().clone(),
            dimensions,
            channels,
            flags,
            flat_gain,
        })
    }

    /// Each channel's samples, read in place from the map.
    pub(crate) fn planes(&self) -> impl Iterator<Item = &[f32]> {
        let samples = self.dimensions.pixel_count();
        self.channels
            .iter()
            .map(move |plane| plane.chunk(0..samples))
    }

    /// How many pixels hold each flag, for an image that keeps its flags.
    pub(crate) fn flag_counts(&self) -> Option<&FlagCounts> {
        self.flags.as_ref().map(|flags| &flags.counts)
    }

    /// The image read back into RAM, of `cfa_type`: its channels, flags, flat gain and metadata,
    /// each plane copied out of its map front to back.
    pub(crate) fn into_image<I: StackableImage>(self, cfa_type: Option<CfaType>) -> I {
        let size = self.dimensions.size();
        let planes = self
            .planes()
            .map(|plane| Buffer2::new(size.width, size.height, plane.to_vec()))
            .collect();
        let flags = self.flags.and_then(|flags| {
            let bytes = flags.plane.chunk(0..size.pixel_count()).to_vec();
            PixelFlags::from_counted(Buffer2::new(size.width, size.height, bytes), flags.counts)
        });
        let parts = ImageParts {
            planes,
            flags,
            flat_gain: self.flat_gain.as_ref().map(StoredGain::to_flat_gain),
        };
        I::from_parts(parts, cfa_type, self.metadata)
    }

    /// The image as the reference frame of a registered stack, every plane where it lies: its
    /// quality is the mask of its flags [`FrameQuality::for_reference`] reads, which holds no
    /// plane, so nothing is written.
    pub(crate) fn into_reference_frame(self, admitted: Admitted) -> StoredFrame {
        let quality = FrameQuality::for_reference(self.flag_counts());
        WrittenFrame {
            image: self,
            samples: None,
            quality,
        }
        .into_frame(admitted)
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::frame_store::stored_gain::StoredGain;
    use crate::frame_store::stored_image::StoredImage;

    impl StoredImage {
        /// Drop the image's pages from the page cache, its channels', its flags' and its gain's,
        /// so its next read comes from the disk; whether each plane went, which a resident one
        /// does not.
        pub(crate) fn evict(&self) -> bool {
            let flags = self.flags.as_ref().is_none_or(|flags| flags.plane.evict());
            self.channels
                .iter()
                .chain(self.flat_gain.iter().flat_map(StoredGain::planes))
                .fold(flags, |all, plane| plane.evict() & all)
        }
    }
}

#[cfg(test)]
mod tests {
    use common::{MappedFile, TempDir};
    use imaginarium::Buffer2;

    use crate::frame_store::run_scratch::RunScratch;
    use crate::frame_store::stored_image::StoredImage;
    use crate::image::channel_layout::ChannelLayout;
    use crate::image::image_dimensions::ImageDimensions;
    use crate::image::linear_image::LinearImage;
    use crate::image::pixel_flags::PixelFlags;
    use crate::math::size2us::Size2us;

    /// A parked image leaves the page cache whole where the system evicts, its flags with its
    /// channels, and loads back as it was spilled.
    #[test]
    fn an_evicted_image_leaves_every_plane_on_the_disk() {
        let dir = TempDir::new("stored_image_evict");
        let scratch = RunScratch::create_evictable(dir.path()).unwrap();
        let size = Size2us::new(512, 512);
        let mut image = LinearImage::from_planar_channels(
            ImageDimensions::new(size, ChannelLayout::Rgb),
            [0.25f32, 0.5, 0.75].map(|level| vec![level; size.pixel_count()]),
        );
        let bytes: Vec<u8> = (0..size.pixel_count()).map(|idx| (idx % 2) as u8).collect();
        image.set_flags(PixelFlags::from_bytes(Buffer2::new(
            size.width,
            size.height,
            bytes.clone(),
        )));
        let parked = StoredImage::write(&image, image.flags(), &scratch).unwrap();
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
