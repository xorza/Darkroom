//! One frame as the combine engine sees it.

use std::fmt::Debug;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame_store::cache_key::CacheKey;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_spill::{Committed, FrameSpill};
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::stackable_image::StackableImage;
use crate::frame_store::stored_plane::StoredPlane;
use crate::io::image::image_dimensions::ImageDimensions;

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
            StoredPlane::map(&path)
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
            .map(|channel| StoredPlane::map(&spill.channel_path(channel)))
            .collect::<Result<_, _>>()?;
        let quality = if carries_quality {
            FrameQuality::read_spilled(|plane| StoredPlane::map(&spill.quality_path(plane)))?
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
