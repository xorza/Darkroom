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
use crate::frame_store::stratified_samples::StratifiedSamples;
use crate::io::image::cfa::CfaType;
use crate::io::image::image_dimensions::ImageDimensions;
use crate::io::image::pixel_flags::{PixelFlags, QualityFlags};

/// One frame as the combine engine sees it: its channel planes, the per-pixel quality it carries
/// if a warp produced one or its source declared pixels with no measurement, its flags when its
/// quality is a mask of them or it carries any but `NO_DATA` (which quality planes already hold),
/// the gain a flat applied to its pixels, and the statistics measured on the source before any
/// interpolation.
#[derive(Debug)]
pub(crate) struct StoredFrame {
    pub(crate) channels: ArrayVec<StoredPlane, 3>,
    /// Each slot's [`StratifiedSamples`], gathered as a frame of no quality went to disk, where
    /// reading them from its planes would read every page; `None` for a frame in memory, or one
    /// whose quality makes normalization sample the pixels the frames share instead.
    pub(crate) samples: Option<ArrayVec<StoredPlane, 3>>,
    pub(crate) quality: FrameQuality<StoredPlane>,
    pub(crate) flags: Option<StoredPlane<u8>>,
    pub(crate) flat_gain: Option<StoredGain>,
    pub(crate) source_stats: FrameStats,
}

/// Whether a stored frame of `quality` keeps `flags`: a mask reads them, and otherwise they are
/// kept when they hold any flag but `NO_DATA`.
const fn keeps<P>(flags: &PixelFlags, quality: &FrameQuality<P>) -> bool {
    quality.mask().is_some() || flags.contains_other_than(QualityFlags::NO_DATA)
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
            .filter(|flags| keeps(flags, &quality))
            .map(|flags| StoredPlane::Memory(flags.into_buffer()));
        Self {
            channels: planes.into_iter().map(StoredPlane::Memory).collect(),
            samples: None,
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
        let samples = quality
            .is_none()
            .then(|| Self::spill_samples(store, image))
            .transpose()?;
        let quality =
            quality.try_map(|plane, buffer| store.store_quality(plane, buffer.pixels()))?;
        let flags = image
            .flags()
            .filter(|flags| keeps(flags, &quality))
            .map(|flags| store.store_flags(flags.bytes()))
            .transpose()?;
        let flat_gain = image
            .flat_gain()
            .map(|gain| StoredGain::spill(store, gain))
            .transpose()?;
        Ok(Self {
            channels,
            samples,
            quality,
            flags,
            flat_gain,
            source_stats,
        })
    }

    /// Each slot's [`StratifiedSamples`] of `image`, written to `store`.
    fn spill_samples(
        store: &impl PlaneStore,
        image: &impl StackableImage,
    ) -> Result<ArrayVec<StoredPlane, 3>, FrameStoreError> {
        let size = image.dimensions().size();
        let mosaic = image.cfa_type().filter(CfaType::is_mosaic);
        let slots = mosaic.map_or(image.dimensions().channels(), |cfa| cfa.num_colors());
        (0..slots)
            .map(|slot| {
                let plane = image.channel(if mosaic.is_some() { 0 } else { slot });
                let samples = StratifiedSamples::new(size, mosaic.as_ref(), slot).gather(plane);
                store.store_samples(slot, &samples)
            })
            .collect()
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
        debug_assert!(
            quality.coverage().is_none(),
            "a kept frame is unwarped, and so carries no quality planes"
        );
        let frame = Self::spill(spill, image, quality, source_stats)?;
        let carries = match (quality.mask(), &frame.flags) {
            (Some(_), _) => Carries::NullMask,
            (None, Some(_)) => Carries::Flags,
            (None, None) => Carries::Channels,
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
            || (carries.flags() && !spill.flags_on_disk(dimensions))
        {
            return Ok(None);
        }
        let channels = (0..dimensions.channels())
            .map(|channel| StoredPlane::map(&spill.channel_path(channel)))
            .collect::<Result<_, _>>()?;
        let quality = match carries {
            Carries::NullMask => FrameQuality::Mask {
                excluded: QualityFlags::NO_DATA,
            },
            Carries::Channels | Carries::Flags => FrameQuality::None,
        };
        let flags = carries
            .flags()
            .then(|| StoredPlane::map(&spill.flags_path()))
            .transpose()?;
        Ok(Some(Self {
            channels,
            samples: None,
            quality,
            flags,
            flat_gain: None,
            source_stats,
        }))
    }
}
