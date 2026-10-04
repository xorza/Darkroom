//! A calibrated image stored between detection and registration.

use std::fmt::Debug;

use arrayvec::ArrayVec;

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::plane_store::PlaneStore;
use crate::frame_store::stored_plane::StoredPlane;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::io::image::pixel_flags::PixelFlags;

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

    pub(crate) fn load(&self) -> LinearImage {
        let sample_count = self.dimensions.pixel_count();
        let planes = self
            .channels
            .iter()
            .map(|plane| plane.chunk(0, sample_count).to_vec());
        let mut image = LinearImage::from_planar_channels(self.dimensions, planes);
        image.metadata = self.metadata.clone();
        image.flags = self.flags.as_ref().map(|bytes| {
            PixelFlags::from_bytes(self.dimensions.size(), bytes.chunk(0, sample_count))
        });
        image
    }
}
