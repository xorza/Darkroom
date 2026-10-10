//! One frame as the combine engine sees it.

use std::fmt::Debug;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame_store::cache_key::CacheKey;
use crate::frame_store::error::FrameStoreError;
use crate::frame_store::frame_quality::FrameQuality;
use crate::frame_store::frame_spill::{Carries, Committed, FrameSpill};
use crate::frame_store::frame_stats::FrameStats;
use crate::frame_store::plane_store::PlaneStore;
use crate::frame_store::stackable_image::{ImageParts, StackableImage};
use crate::frame_store::stored_gain::StoredGain;
use crate::frame_store::stored_plane::StoredPlane;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};

/// One frame as the combine engine sees it: its channel planes, the per-pixel quality it carries
/// if a warp produced one or its source declared pixels with no measurement, its flags when it
/// carries any but `NO_DATA` (which the quality planes already hold), the gain a flat applied to
/// its pixels, and the statistics measured on the source before any interpolation.
#[derive(Debug)]
pub(crate) struct StoredFrame {
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    pub(crate) quality: FrameQuality<StoredPlane>,
    pub(crate) flags: Option<StoredPlane<u8>>,
    pub(crate) flat_gain: Option<StoredGain>,
    pub(crate) source_stats: FrameStats,
}

/// The flags a stored frame keeps: those of an image that carries any but `NO_DATA`.
fn kept_flags(flags: Option<&PixelFlags>) -> Option<&PixelFlags> {
    flags.filter(|flags| flags.contains_other_than(QualityFlags::NO_DATA))
}

impl StoredFrame {
    pub(crate) fn from_memory(
        image: impl StackableImage,
        quality: FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Self {
        let flat_gain = image.flat_gain().map(StoredGain::from_memory);
        let ImageParts { planes, flags } = image.into_parts();
        let flags = flags
            .filter(|flags| flags.contains_other_than(QualityFlags::NO_DATA))
            .map(|flags| StoredPlane::Memory(flags.into_buffer()));
        Self {
            channels: planes.into_iter().map(StoredPlane::Memory).collect(),
            quality: quality.map(StoredPlane::Memory),
            flags,
            flat_gain,
            source_stats,
        }
    }

    /// Write the frame's channels, quality planes and flags to `store` and memory-map them back.
    ///
    /// Borrows everything it writes: the caller keeps its buffers, which is what lets the warp
    /// stage hand the same ones to the next frame rather than allocating a set that has to be
    /// faulted in from scratch.
    pub(crate) fn spill(
        store: &impl PlaneStore,
        image: &impl StackableImage,
        quality: &FrameQuality<Buffer2<f32>>,
        source_stats: FrameStats,
    ) -> Result<Self, FrameStoreError> {
        let channels = (0..image.dimensions().channels())
            .map(|channel| store.store_channel(channel, image.channel(channel)))
            .collect::<Result<_, _>>()?;
        let quality =
            quality.try_map(|plane, buffer| store.store_quality(plane, buffer.pixels()))?;
        let flags = kept_flags(image.flags())
            .map(|flags| store.store_flags(flags.bytes()))
            .transpose()?;
        let flat_gain = image
            .flat_gain()
            .map(|gain| StoredGain::spill(store, gain))
            .transpose()?;
        Ok(Self {
            channels,
            quality,
            flags,
            flat_gain,
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
        debug_assert!(
            image.metadata().flat_gain.is_none(),
            "a kept frame is as its source decodes, and no flat divided that"
        );
        let frame = Self::spill(spill, image, quality, source_stats)?;
        let carries = Carries {
            quality: !quality.is_none(),
            flags: frame.flags.is_some(),
        };
        spill.commit(key, carries, &frame.source_stats)?;
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
            carries,
        }) = spill.committed(key)
        else {
            return Ok(None);
        };
        if !spill.channels_on_disk(dimensions)
            || (carries.quality && !spill.quality_on_disk(dimensions))
            || (carries.flags && !spill.flags_on_disk(dimensions))
        {
            return Ok(None);
        }
        let channels = (0..dimensions.channels())
            .map(|channel| StoredPlane::map(&spill.channel_path(channel)))
            .collect::<Result<_, _>>()?;
        let quality = if carries.quality {
            FrameQuality::read_spilled(|plane| StoredPlane::map(&spill.quality_path(plane)))?
        } else {
            FrameQuality::None
        };
        let flags = carries
            .flags
            .then(|| StoredPlane::map(&spill.flags_path()))
            .transpose()?;
        Ok(Some(Self {
            channels,
            quality,
            flags,
            flat_gain: None,
            source_stats,
        }))
    }
}
