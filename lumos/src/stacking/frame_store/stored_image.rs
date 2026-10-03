//! A calibrated image stored between detection and registration.

use std::fmt::Debug;

use arrayvec::ArrayVec;
use memmap2::Mmap;

use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::io::image::null_mask::NullMask;
use crate::stacking::frame_store::error::FrameStoreError;
use crate::stacking::frame_store::frame_spill::{self, FrameSpill, write_file};
use crate::stacking::frame_store::stored_plane::StoredPlane;

/// A calibrated image stored on disk between detection and registration.
#[derive(Debug)]
pub(crate) struct StoredImage {
    pub(crate) metadata: ImageMetadata,
    pub(crate) dimensions: ImageDimensions,
    channels: ArrayVec<StoredPlane, 3>,
    /// The words of the image's [`NullMask`], for a source that declared nulls. Spilled with the
    /// channels: without it the fill under every null reads back as a measurement.
    nulls: Option<Mmap>,
}

impl StoredImage {
    /// Write `image`'s channels and null mask to `spill`'s files and memory-map them back.
    pub(crate) fn spill(
        spill: &FrameSpill<'_>,
        image: &LinearImage,
    ) -> Result<Self, FrameStoreError> {
        let nulls = image
            .nulls
            .as_ref()
            .map(|nulls| {
                let path = spill.nulls_path();
                write_file(&path, bytemuck::cast_slice(nulls.bits().words.as_slice()))?;
                frame_spill::map_file(&path)
            })
            .transpose()?;
        Ok(Self {
            metadata: image.metadata.clone(),
            dimensions: image.dimensions(),
            channels: spill.spill_channels(image)?,
            nulls,
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
        image.nulls = self
            .nulls
            .as_ref()
            .map(|words| NullMask::from_words(self.dimensions.size(), bytemuck::cast_slice(words)));
        image
    }
}
