//! A calibrated image stored between detection and registration.

use std::fmt::Debug;

use arrayvec::ArrayVec;
use memmap2::Mmap;

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_spill::{self, FrameSpill, write_file};
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
    flags: Option<Mmap>,
}

impl StoredImage {
    /// Write `image`'s channels and flags to `spill`'s files and memory-map them back.
    pub(crate) fn spill(
        spill: &FrameSpill<'_>,
        image: &LinearImage,
    ) -> Result<Self, FrameStoreError> {
        let flags = image
            .flags
            .as_ref()
            .map(|flags| {
                let path = spill.flags_path();
                write_file(&path, flags.bytes())?;
                frame_spill::map_file(&path)
            })
            .transpose()?;
        Ok(Self {
            metadata: image.metadata.clone(),
            dimensions: image.dimensions(),
            channels: spill.spill_channels(image)?,
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
        image.flags = self
            .flags
            .as_ref()
            .map(|bytes| PixelFlags::from_bytes(self.dimensions.size(), bytes));
        image
    }
}
