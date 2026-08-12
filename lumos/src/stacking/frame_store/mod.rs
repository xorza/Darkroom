//! Memory planning and RAM/mmap storage shared by stacking stages.

pub(crate) mod error;
pub(crate) mod frame_quality;
pub(crate) mod frame_stats;
pub(crate) mod spill;
pub(crate) mod stored_plane;

use std::path::Path;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::null_mask::NullMask;
use crate::stacking::frame_store::error::FrameStoreError;
use crate::stacking::frame_store::frame_quality::FrameQuality;
use crate::stacking::frame_store::frame_stats::FrameStats;
use crate::stacking::frame_store::spill::{FrameSpill, spill_channels, write_plane};
use crate::stacking::frame_store::stored_plane::StoredPlane;

/// Image operations needed by the shared frame store.
pub(crate) trait StackableImage: Send + Sync + std::fmt::Debug + Sized {
    fn dimensions(&self) -> ImageDimensions;
    fn channel(&self, channel: usize) -> &[f32];
    fn metadata(&self) -> &ImageMetadata;
    fn load(path: &Path, context: &LoadContext) -> Result<Self, ImageError>;

    fn quantization_sigma(&self) -> Option<f32> {
        None
    }

    /// Which of the image's pixels carry no measurement, for a source that declared any.
    ///
    /// No default: both implementors know the answer, and a default of "none" would let a decoder
    /// that starts recording nulls have them silently dropped here.
    fn nulls(&self) -> Option<&NullMask>;

    fn peek_dimensions(_path: &Path, _context: &LoadContext) -> Option<ImageDimensions> {
        None
    }

    fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3>;
}

/// One frame as the combine engine sees it: its channel planes, the per-pixel quality it carries
/// if a warp produced one or its source declared pixels with no measurement, and the statistics
/// measured on the source before any interpolation.
#[derive(Debug)]
pub(crate) struct StoredFrame {
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    pub(crate) quality: FrameQuality<StoredPlane>,
    pub(crate) source_stats: FrameStats,
}

impl StoredFrame {
    pub(crate) fn from_memory(
        image: impl StackableImage,
        quality: FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Self {
        let channels = image
            .into_planes()
            .into_iter()
            .map(StoredPlane::Memory)
            .collect();
        Self {
            channels,
            quality: quality.map(StoredPlane::Memory),
            source_stats,
        }
    }

    /// Write the frame's channels and quality planes under `directory` and memory-map them back.
    ///
    /// Borrows everything it writes: the caller keeps its buffers, which is what lets the warp
    /// stage hand the same ones to the next frame rather than allocating a set that has to be
    /// faulted in from scratch.
    pub(crate) fn spill(
        directory: &Path,
        name: &str,
        image: &impl StackableImage,
        quality: &FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Result<Self, FrameStoreError> {
        let spill = FrameSpill::new(directory, name);
        let channels = spill_channels(spill, image)?;
        let quality = quality.try_map(|kind, plane| {
            let path = spill.quality_path(kind);
            write_plane(&path, plane.pixels())?;
            StoredPlane::map(path)
        })?;
        Ok(Self {
            channels,
            quality,
            source_stats,
        })
    }
}

/// A calibrated image stored on disk between detection and registration.
#[derive(Debug)]
pub(crate) struct StoredImage {
    pub(super) metadata: ImageMetadata,
    pub(super) dimensions: ImageDimensions,
    channels: ArrayVec<StoredPlane, 3>,
}

impl StoredImage {
    /// Write `image`'s channels under `directory` and memory-map them back.
    pub(crate) fn spill(
        directory: &Path,
        name: &str,
        image: &LinearImage,
    ) -> Result<Self, FrameStoreError> {
        let dimensions = image.dimensions();
        Ok(Self {
            metadata: image.metadata.clone(),
            dimensions,
            channels: spill_channels(FrameSpill::new(directory, name), image)?,
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
        image
    }
}

#[cfg(test)]
mod tests;
