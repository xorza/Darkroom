//! Memory planning and RAM/mmap storage shared by stacking stages.

pub(crate) mod cache_key;
pub(crate) mod error;
pub(crate) mod frame_facts;
pub(crate) mod frame_quality;
pub(crate) mod frame_spill;
pub(crate) mod frame_stats;
pub(crate) mod spill_directory;
pub(crate) mod stored_plane;

use std::fmt::Debug;
use std::path::Path;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;
use memmap2::Mmap;

use crate::io::image::cfa::{CfaFrameInfo, CfaType};
use crate::io::image::error::ImageError;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::linear::LinearImage;
use crate::io::image::load_context::LoadContext;
use crate::io::image::null_mask::NullMask;
use crate::memory;
use crate::stacking::frame_store::cache_key::CacheKey;
use crate::stacking::frame_store::cache_key::DecoderKind;
use crate::stacking::frame_store::error::FrameStoreError;
use crate::stacking::frame_store::frame_quality::FrameQuality;
use crate::stacking::frame_store::frame_spill::{Committed, FrameSpill, write_file};
use crate::stacking::frame_store::frame_stats::FrameStats;
use crate::stacking::frame_store::stored_plane::StoredPlane;

/// Image operations needed by the shared frame store.
pub(crate) trait StackableImage: Send + Sync + Debug + Sized {
    /// The decoder [`Self::load`] runs, which a kept cache records beside the planes it wrote.
    const DECODER: DecoderKind;

    fn dimensions(&self) -> ImageDimensions;
    fn channel(&self, channel: usize) -> &[f32];
    fn metadata(&self) -> &ImageMetadata;
    /// The mosaic pattern of an undemosaiced sensor frame; `None` for any other image.
    fn cfa_type(&self) -> Option<CfaType>;
    fn load(path: &Path, context: &LoadContext) -> Result<Self, ImageError>;

    fn quantization_sigma(&self) -> Option<f32> {
        None
    }

    /// Which of the image's pixels carry no measurement, for a source that declared any.
    ///
    /// No default: both implementors know the answer, and a default of "none" would let a decoder
    /// that starts recording nulls have them silently dropped here.
    fn nulls(&self) -> Option<&NullMask>;

    /// What a header alone settles about a frame, for a format that can answer without decoding.
    /// `None` when it cannot, which leaves the caller to decode the frame and read the answer off
    /// it — see [`FramePeek::of_decoded`].
    fn peek(_path: &Path, _context: &LoadContext) -> Option<FramePeek> {
        None
    }

    fn into_planes(self) -> ArrayVec<Buffer2<f32>, 3>;
}

/// What one frame is worth to a memory estimate, before the rest of the set is read.
///
/// Sizing a run needs the geometry and whether the frame carries the two quality planes a masked
/// one does. A header answers the first exactly and the second only sometimes, so the second is
/// deliberately allowed to over-report: reserving planes a frame turns out not to carry costs a run
/// some concurrency, while missing planes it does carry overcommits the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FramePeek {
    pub(crate) dimensions: ImageDimensions,
    pub(crate) may_carry_nulls: bool,
}

impl FramePeek {
    /// What a frame already in hand settles — exactly, since its mask is right there.
    pub(crate) fn of_decoded(image: &impl StackableImage) -> Self {
        Self {
            dimensions: image.dimensions(),
            may_carry_nulls: image.nulls().is_some(),
        }
    }

    /// Bytes one such frame occupies once resident: its own pixels, plus the quality planes if it
    /// may carry them.
    pub(crate) fn resident_bytes(self) -> usize {
        let quality = if self.may_carry_nulls {
            memory::quality_plane_bytes(self.dimensions)
        } else {
            0
        };
        memory::frame_bytes(self.dimensions) + quality
    }
}

impl From<CfaFrameInfo> for FramePeek {
    /// A CFA peek answers everything this needs and the demosaic kind besides, which no memory
    /// estimate reads.
    fn from(info: CfaFrameInfo) -> Self {
        Self {
            dimensions: info.dimensions,
            may_carry_nulls: info.may_carry_nulls,
        }
    }
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

    /// Write the frame's channels and quality planes to `spill`'s files and memory-map them back.
    ///
    /// Borrows everything it writes: the caller keeps its buffers, which is what lets the warp
    /// stage hand the same ones to the next frame rather than allocating a set that has to be
    /// faulted in from scratch.
    pub(crate) fn spill(
        spill: &FrameSpill<'_>,
        image: &impl StackableImage,
        quality: &FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Result<Self, FrameStoreError> {
        let channels = spill.spill_channels(image)?;
        let quality = quality.try_map(|plane, buffer| {
            let path = spill.quality_path(plane);
            StoredPlane::write(&path, buffer.pixels())?;
            StoredPlane::map(path)
        })?;
        Ok(Self {
            channels,
            quality,
            source_stats,
        })
    }

    /// [`Self::spill`], then commit the files as a kept frame decoded under `key`.
    pub(crate) fn cache(
        spill: &FrameSpill<'_>,
        key: CacheKey,
        image: &impl StackableImage,
        quality: &FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Result<Self, FrameStoreError> {
        let frame = Self::spill(spill, image, quality, source_stats)?;
        spill.commit(key, !quality.is_none(), &frame.source_stats)?;
        Ok(frame)
    }

    /// The frame [`Self::cache`] committed to `spill`'s files under `key`, mapped; `None` when
    /// there is none whole to reuse — no commit under this key, or a plane missing or of another
    /// size.
    pub(crate) fn reuse(
        spill: &FrameSpill<'_>,
        key: CacheKey,
        dimensions: ImageDimensions,
    ) -> Result<Option<Self>, FrameStoreError> {
        let Some(Committed {
            stats: source_stats,
            carries_quality,
        }) = spill.committed(key)
        else {
            return Ok(None);
        };
        if !spill.channels_on_disk(dimensions)
            || (carries_quality && !spill.quality_on_disk(dimensions))
        {
            return Ok(None);
        }
        let channels = (0..dimensions.channels())
            .map(|channel| StoredPlane::map(spill.channel_path(channel)))
            .collect::<Result<_, _>>()?;
        let quality = if carries_quality {
            FrameQuality::read_spilled(|plane| StoredPlane::map(spill.quality_path(plane)))?
        } else {
            FrameQuality::None
        };
        Ok(Some(Self {
            channels,
            quality,
            source_stats,
        }))
    }
}

/// A calibrated image stored on disk between detection and registration.
#[derive(Debug)]
pub(crate) struct StoredImage {
    pub(super) metadata: ImageMetadata,
    pub(super) dimensions: ImageDimensions,
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

#[cfg(test)]
mod tests;
